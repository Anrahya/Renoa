package live.renoa.app.push

import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.agentAddress
import live.renoa.app.agents.chatTaskId
import kotlinx.coroutines.CancellationException
import live.renoa.rcp.Disconnected
import live.renoa.rcp.ExecutionEventKind
import live.renoa.rcp.ExecutionTerminal
import live.renoa.rcp.PendingTextCommand
import live.renoa.rcp.ProtocolViolation
import live.renoa.rcp.RcpErrorCode
import live.renoa.rcp.RcpException
import live.renoa.rcp.RcpState
import live.renoa.rcp.RcpSurface
import live.renoa.rcp.TaskEvent
import live.renoa.rcp.TaskEventKind
import live.renoa.rcp.TaskSummary
import java.util.concurrent.ConcurrentHashMap
import kotlin.uuid.Uuid

/** What a task's newly read records have for the owner; [sequence] is the record that says it. */
internal sealed interface News {
    val sequence: Long

    /** The agent's latest reply with words in it. */
    data class Replied(val text: String, override val sequence: Long) : News

    /** The latest run failed. */
    data class Stuck(override val sequence: Long) : News
}

/**
 * What [read] has for the owner, or null when nothing in it does. Only a reply with words in it and
 * a failed run count, the records Renoa wakes the phone for (`push_wake.rs`); whichever came last
 * is the news. Every kind is decided here without a wildcard, so a new one needs a decision.
 */
internal fun news(read: List<TaskEvent>): News? {
    var latest: News? = null
    for (event in read) {
        when (val kind = event.kind) {
            is TaskEventKind.CommandSubmitted -> {}
            is TaskEventKind.ExecutionRecorded ->
                when (val activity = kind.event.kind) {
                    is ExecutionEventKind.AssistantMessage -> if (activity.text.isNotBlank()) latest = News.Replied(activity.text.trim(), event.sequence)
                    is ExecutionEventKind.ExecutionTerminated ->
                        when (activity.terminal) {
                            is ExecutionTerminal.Failed -> latest = News.Stuck(event.sequence)
                            ExecutionTerminal.Completed, is ExecutionTerminal.Cancelled -> {}
                        }
                    ExecutionEventKind.ExecutionStarted, ExecutionEventKind.TurnStarted, is ExecutionEventKind.ToolStarted, is ExecutionEventKind.ToolFinished -> {}
                }
        }
    }
    return latest
}

/**
 * One notification: [news] from the task [taskId], whose agent is called [agent] when the app knows
 * it. [chat] is the agent whose chat the task is, which a tap opens, or null for another task.
 */
internal data class Notice(val taskId: Uuid, val agent: String?, val chat: String?, val news: News)

/** Who a task speaks for, as far as the app knows: the agent's [name], and its [chat] when the task is that chat. */
internal data class Sender(val name: String?, val chat: String?)

/**
 * The agent [task] runs for, found by its target among the [agents] the app keeps; [Sender.chat] is
 * set only when the task is that agent's chat in the app.
 */
internal fun sender(task: TaskSummary, agents: List<AgentSummary>): Sender {
    val agent = agents.firstOrNull { agentAddress(it.id)?.second == task.target } ?: return Sender(null, null)
    val chat = agentAddress(agent.id)?.let { (node, target) -> chatTaskId(node, target) }
    return Sender(agent.name, agent.id.takeIf { chat == task.taskId })
}

/**
 * A wake-up's connection state: cursors come from [saved] until this connection reads past them, and
 * nothing is ever sent, so the outbox is always empty.
 */
private class WakeState(private val saved: WakeCursors) : RcpState {
    private val read = ConcurrentHashMap<Uuid, Long>()

    override suspend fun cursor(taskId: Uuid): Long? = read[taskId] ?: saved.cursor(taskId)

    override suspend fun advanceCursor(taskId: Uuid, sequence: Long) {
        read[taskId] = sequence
    }

    override suspend fun enqueueCommand(command: PendingTextCommand) = throw IllegalStateException("a wake-up sends nothing")

    override suspend fun pendingCommands(): List<PendingTextCommand> = emptyList()

    override suspend fun removeCommand(commandId: Uuid) {}
}

/**
 * What the app does after Renoa woke it: connect as this phone, [register] its push token, list the
 * tasks, read each one whose `throughSequence` passed its cursor in [cursors], and [post] what was
 * new, unless that chat is [onScreen]. A task's cursor moves only after its notice was posted or
 * passed over, so a wake-up cut short shows those records on the next one.
 *
 * A failure in one task, or in registering, does not stop the others; [run] then returns false, for
 * the caller to run it again. A failure that ends the connection is followed by a new one, and the
 * task is tried once more; one that ends it twice is left for the next run. Each connection reads from
 * the kept cursors afresh, so what a dropped one read and did not show is read again. Without a
 * baseline there is nothing to read from, so it returns false too. A failure to connect or to list
 * the tasks throws. [connect] makes a client over the state it is given; [run] connects and closes
 * each one it makes.
 */
internal class WakeUp(
    private val cursors: WakeCursors,
    private val connect: (RcpState) -> RcpSurface,
    private val register: suspend (RcpSurface) -> Unit,
    private val sender: (TaskSummary) -> Sender,
    private val onScreen: (Uuid) -> Boolean,
    private val post: (Notice) -> Unit,
) {
    /** Returns whether every step and every task went through. */
    suspend fun run(): Boolean {
        var surface = connect(WakeState(cursors))
        try {
            surface.connect()
            var complete = attempt { register(surface) } == null
            if (!cursors.baselined()) return false
            for (task in surface.listTasks()) {
                val through = task.throughSequence ?: continue
                var ended = 0
                while (true) {
                    val failure = attempt { if ((cursors.cursor(task.taskId) ?: -1) < through) catchUp(surface, task) } ?: break
                    if (!failure.endsTheConnection && alive(surface)) {
                        complete = false
                        break
                    }
                    surface.close()
                    // A new state too: the old one moved past records the dropped connection read and nothing showed.
                    surface = connect(WakeState(cursors))
                    surface.connect()
                    if (++ended == CONNECTIONS_PER_TASK) {
                        complete = false
                        break
                    }
                }
            }
            return complete
        } finally {
            surface.close()
        }
    }

    private suspend fun catchUp(surface: RcpSurface, task: TaskSummary) {
        val read = mutableListOf<TaskEvent>()
        try {
            surface.attach(task.taskId) { event -> synchronized(read) { read += event } }
        } catch (gone: RcpException) {
            // Deleted since it was listed.
            if (gone.code == RcpErrorCode.NotFound && gone.requestId != null) return
            throw gone
        }
        // A record arriving live from here on is left to the next wake-up, which Renoa sends for it.
        val replayed = synchronized(read) { read.toList() }
        news(replayed)?.let { said ->
            // Checked last, so a chat the owner opened, or read, while it was replayed is not shown.
            val alreadyRead = (cursors.cursor(task.taskId) ?: -1) >= said.sequence
            if (!alreadyRead && !onScreen(task.taskId)) sender(task).let { from -> post(Notice(task.taskId, from.name, from.chat, said)) }
        }
        replayed.lastOrNull()?.let { cursors.advance(task.taskId, it.sequence) }
    }

    /** Whether [surface] still answers, as after a failure that did not say the connection ended. */
    private suspend fun alive(surface: RcpSurface): Boolean = attempt { surface.probe() } == null

    /** Runs [step]; returns what it failed with, or null. */
    private suspend fun attempt(step: suspend () -> Unit): Exception? =
        try {
            step()
            null
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: Exception) {
            failure
        }

    private companion object {
        /** Connections one task may end in a run before it is left for the next. */
        const val CONNECTIONS_PER_TASK = 2

        /** Whether the connection is gone after this, so later requests on it would fail too. */
        val Exception.endsTheConnection: Boolean
            get() = this is Disconnected || this is ProtocolViolation || (this is RcpException && requestId == null)
    }
}

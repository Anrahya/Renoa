package live.renoa.app.push

import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.agentAddress
import live.renoa.app.agents.chatTaskId
import kotlinx.coroutines.CancellationException
import live.renoa.rcp.ExecutionEventKind
import live.renoa.rcp.ExecutionTerminal
import live.renoa.rcp.PendingTextCommand
import live.renoa.rcp.RcpErrorCode
import live.renoa.rcp.RcpException
import live.renoa.rcp.RcpState
import live.renoa.rcp.RcpSurface
import live.renoa.rcp.TaskEvent
import live.renoa.rcp.TaskEventKind
import live.renoa.rcp.TaskSummary
import java.util.concurrent.ConcurrentHashMap
import kotlin.uuid.Uuid

/** What a task's newly read records have for the owner. */
internal sealed interface News {
    /** The agent's latest reply with words in it. */
    data class Replied(val text: String) : News

    /** The latest run failed. */
    data object Stuck : News
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
                    is ExecutionEventKind.AssistantMessage -> if (activity.text.isNotBlank()) latest = News.Replied(activity.text.trim())
                    is ExecutionEventKind.ExecutionTerminated ->
                        when (activity.terminal) {
                            is ExecutionTerminal.Failed -> latest = News.Stuck
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
 * the caller to run it again. Without a baseline there is nothing to read from, so it returns false
 * too. A failure to connect or to list the tasks throws. [connect] makes the client over the state it
 * is given; [run] connects it and closes it.
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
        val state = WakeState(cursors)
        val surface = connect(state)
        try {
            surface.connect()
            var complete = attempt { register(surface) }
            if (!cursors.baselined()) return false
            for (task in surface.listTasks()) {
                val through = task.throughSequence ?: continue
                complete = attempt { if ((cursors.cursor(task.taskId) ?: -1) < through) catchUp(surface, task) } && complete
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
        news(replayed)?.let {
            if (!onScreen(task.taskId)) sender(task).let { from -> post(Notice(task.taskId, from.name, from.chat, it)) }
        }
        replayed.lastOrNull()?.let { cursors.advance(task.taskId, it.sequence) }
    }

    /** Runs [step]; returns whether it went through. */
    private suspend fun attempt(step: suspend () -> Unit): Boolean =
        try {
            step()
            true
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Exception) {
            false
        }
}

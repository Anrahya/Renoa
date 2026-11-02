package live.renoa.app.push

import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.agentAddress
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

/** One notification: [news] from the task [taskId], whose agent is called [agent] when the app knows it. */
internal data class Notice(val taskId: Uuid, val agent: String?, val news: News)

/** The name the app keeps for the agent [task] runs for, found by its target among [agents], or null. */
internal fun agentName(task: TaskSummary, agents: List<AgentSummary>): String? = agents.firstOrNull { agentAddress(it.id)?.second == task.target }?.name

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
 * [connect] makes the client over the state it is given; [run] connects it and closes it.
 */
internal class WakeUp(
    private val cursors: WakeCursors,
    private val connect: (RcpState) -> RcpSurface,
    private val register: suspend (RcpSurface) -> Unit,
    private val agentName: (TaskSummary) -> String?,
    private val onScreen: (Uuid) -> Boolean,
    private val post: (Notice) -> Unit,
) {
    suspend fun run() {
        val state = WakeState(cursors)
        val surface = connect(state)
        try {
            surface.connect()
            register(surface)
            // Taken by registering, or by the app before; one that could not be written is taken next time.
            if (!cursors.baselined()) return
            for (task in surface.listTasks()) {
                val through = task.throughSequence ?: continue
                if ((cursors.cursor(task.taskId) ?: -1) >= through) continue
                val read = mutableListOf<TaskEvent>()
                try {
                    surface.attach(task.taskId) { event -> synchronized(read) { read += event } }
                } catch (gone: RcpException) {
                    // Deleted since it was listed.
                    if (gone.code == RcpErrorCode.NotFound && gone.requestId != null) continue
                    throw gone
                }
                // A record arriving live from here on is left to the next wake-up, which Renoa sends for it.
                val replayed = synchronized(read) { read.toList() }
                news(replayed)?.let { if (!onScreen(task.taskId)) post(Notice(task.taskId, agentName(task), it)) }
                replayed.lastOrNull()?.let { cursors.advance(task.taskId, it.sequence) }
            }
        } finally {
            surface.close()
        }
    }
}

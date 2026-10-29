package live.renoa.app.agents

import live.renoa.rcp.RcpErrorCode
import live.renoa.rcp.RcpException
import live.renoa.rcp.RcpState
import live.renoa.rcp.RcpSurface
import kotlin.time.Duration
import kotlin.time.Duration.Companion.minutes
import kotlin.time.TimeMark
import kotlin.time.TimeSource
import kotlin.uuid.Uuid

/**
 * Sends again what the outbox in [state] still holds, chat by chat, so a message one chat cannot send
 * holds up no other. A refused message is dropped. Once the coordinator has been answering for
 * [GIVE_UP_AFTER] that it cannot take a message, as when the agent's node is gone, the message is
 * given up: it leaves the outbox. Either way [dropped] tells its chat. Only answers count, so a phone
 * that cannot reach Renoa never gives up a message. The wait is kept in memory and starts again after
 * a launch.
 */
internal class UnsentMessages(
    private val surface: RcpSurface,
    private val state: RcpState,
    private val clock: TimeSource,
    private val dropped: (taskId: Uuid, change: ChatJournal.() -> Unit) -> Unit,
) {
    private val lock = Any()

    /** When the coordinator first answered that it could not take each unsent message. */
    private val notTakenSince = HashMap<Uuid, TimeMark>()

    /** Sends every chat's unsent messages, until an error ends the connection. */
    suspend fun send() {
        for (taskId in state.pendingCommands().map { it.taskId }.distinct()) {
            if (!send(taskId)) return
        }
    }

    /**
     * Sends one chat's unsent messages. The client sends them in order, so a failure is for the first
     * one left; once it is dropped, the next one goes. Returns false once an error ends the connection.
     */
    suspend fun send(taskId: Uuid): Boolean {
        while (true) {
            try {
                val admitted = surface.retryPendingCommands(taskId)
                synchronized(lock) { admitted.forEach(notTakenSince::remove) }
                return true
            } catch (failure: RcpException) {
                if (failure.requestId == null) return false
                val first = state.pendingCommands().firstOrNull { it.taskId == taskId } ?: return true
                val change: ChatJournal.() -> Unit =
                    when {
                        failure.refusesTheRequest -> { { refused(first.commandId) } }
                        notTaken(first.commandId) >= GIVE_UP_AFTER -> { { notSent(first.commandId) } }
                        else -> return true
                    }
                state.removeCommand(first.commandId)
                synchronized(lock) { notTakenSince.remove(first.commandId) }
                dropped(taskId, change)
            }
        }
    }

    /** Notes that the coordinator answered it could not take [commandId], and returns how long it has answered so. */
    fun notTaken(commandId: Uuid): Duration = synchronized(lock) { notTakenSince.getOrPut(commandId) { clock.markNow() } }.elapsedNow()

    companion object {
        val GIVE_UP_AFTER = 1.minutes
    }
}

/**
 * Whether the coordinator answered this request with a refusal that a retry cannot change. An
 * error with no request ends the connection, and one like `node_offline` or `internal` may pass.
 */
internal val RcpException.refusesTheRequest: Boolean
    get() =
        requestId != null &&
            when (code) {
                RcpErrorCode.InvalidMessage, RcpErrorCode.InvalidRole, RcpErrorCode.NotFound, RcpErrorCode.Conflict -> true
                RcpErrorCode.AuthenticationFailed, RcpErrorCode.NodeOffline, RcpErrorCode.Internal,
                RcpErrorCode.ReplayRequired, RcpErrorCode.VersionMismatch,
                -> false
            }

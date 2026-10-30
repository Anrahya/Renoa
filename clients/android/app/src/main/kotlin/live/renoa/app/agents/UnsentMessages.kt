package live.renoa.app.agents

import kotlinx.coroutines.CancellationException
import live.renoa.rcp.PendingTextCommand
import live.renoa.rcp.RcpException
import live.renoa.rcp.RcpState
import live.renoa.rcp.RcpSurface
import kotlin.time.Duration
import kotlin.time.Duration.Companion.minutes
import kotlin.time.TimeMark
import kotlin.time.TimeSource
import kotlin.uuid.Uuid

/**
 * Sends again what the outbox in [state] still holds, each chat's messages in order and apart from
 * every other chat's. A refused message leaves the outbox.
 *
 * Once the coordinator has answered for [GIVE_UP_AFTER] on one connection that it cannot take a
 * message, as when the agent's node is gone, the message is given up: it is not sent again until
 * [retry], and the chat's next message goes. It is given up only while the owner can see its chat
 * ([shown]), so it can be shown as not sent. A dropped connection starts the wait again, so time the
 * phone cannot reach Renoa never counts. A given-up message stays in the outbox, so nothing the owner
 * wrote is lost; after a relaunch it is tried again and its wait starts over.
 *
 * [dropped] tells a chat that its message was refused or given up.
 */
internal class UnsentMessages(
    private val surface: RcpSurface,
    private val state: RcpState,
    private val clock: TimeSource,
    private val shown: (taskId: Uuid) -> Boolean,
    private val dropped: (taskId: Uuid, change: ChatJournal.() -> Unit) -> Unit,
) {
    private val lock = Any()

    /** When the coordinator first answered, on this connection, that it could not take each message. */
    private val notTakenSince = HashMap<Uuid, TimeMark>()
    private val givenUp = HashSet<Uuid>()

    /** Sends every chat's unsent messages, until an error ends the connection. */
    suspend fun send() {
        val pending = state.pendingCommands()
        val held = synchronized(lock) { pending.filter { it.commandId !in givenUp } }
        for ((taskId, messages) in held.groupBy { it.taskId }) {
            if (!send(taskId, messages)) return
        }
    }

    /** Sends one chat's messages in order. Returns false once an error ends the connection. */
    private suspend fun send(taskId: Uuid, messages: List<PendingTextCommand>): Boolean {
        for (message in messages) {
            try {
                surface.resendPending(message)
                forget(message.commandId)
            } catch (failure: RcpException) {
                if (!failure.answered) return false
                if (failure.refusesTheRequest) {
                    state.removeCommand(message.commandId)
                    forget(message.commandId)
                    dropped(taskId) { refused(message.commandId) }
                    continue
                }
                // This chat waits behind it until it is given up.
                if (notTaken(message.commandId) < GIVE_UP_AFTER || !shown(taskId)) return true
                synchronized(lock) {
                    givenUp += message.commandId
                    notTakenSince.remove(message.commandId)
                }
                dropped(taskId) { notSent(message.commandId) }
            }
        }
        return true
    }

    /**
     * Sends a given-up message again now, with its wait started over. A refusal takes it out of the
     * outbox and is thrown; any other failure leaves it to the next send, as any unsent message.
     */
    suspend fun retry(message: PendingTextCommand) {
        forget(message.commandId)
        try {
            surface.resendPending(message)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (failure: RcpException) {
            if (failure.refusesTheRequest) {
                state.removeCommand(message.commandId)
                throw failure
            }
            if (failure.answered) notTaken(message.commandId)
        } catch (lost: Exception) {
            // The connection is down; the next connect sends it.
        }
    }

    /** Notes that the coordinator answered it could not take [commandId], and returns how long it has answered so. */
    fun notTaken(commandId: Uuid): Duration = synchronized(lock) { notTakenSince.getOrPut(commandId) { clock.markNow() } }.elapsedNow()

    /** The connection dropped: what the coordinator answered on it no longer counts toward giving up. */
    fun connectionLost() = synchronized(lock) { notTakenSince.clear() }

    /** The message was admitted, refused, retried or deleted, so nothing about it is kept. */
    fun forget(commandId: Uuid) = synchronized(lock) {
        notTakenSince.remove(commandId)
        givenUp.remove(commandId)
    }

    companion object {
        val GIVE_UP_AFTER = 1.minutes
    }
}

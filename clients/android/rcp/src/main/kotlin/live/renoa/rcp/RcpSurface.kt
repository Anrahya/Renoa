package live.renoa.rcp

import kotlinx.coroutines.Deferred
import kotlin.uuid.Uuid

/** How a surface proves itself on a new connection. The principal and surface role come from the enrollment. */
sealed interface Authentication {
    data class Device(val credentials: DeviceCredentials) : Authentication

    /** A one-use, 60-second connection ticket. Each connection attempt asks for a new one. */
    class Ticket(val get: suspend () -> String) : Authentication
}

/** A command the surface wrote down before sending it, so a lost acknowledgement can be retried exactly. */
data class PendingTextCommand(val commandId: Uuid, val taskId: Uuid, val text: String)

/**
 * What the host app keeps across connections and restarts. The client calls these in order:
 * a command is enqueued before it is sent, and a cursor advances only after the app applied
 * the event, so nothing the app has not stored is ever acknowledged.
 */
interface RcpState {
    suspend fun cursor(taskId: Uuid): Long?

    suspend fun advanceCursor(taskId: Uuid, sequence: Long)

    suspend fun enqueueCommand(command: PendingTextCommand)

    /** Pending commands in the order they were enqueued. */
    suspend fun pendingCommands(): List<PendingTextCommand>

    suspend fun removeCommand(commandId: Uuid)
}

/** A command written to the outbox. [accepted] completes when the coordinator durably admitted it. */
class CommandSubmission(val commandId: Uuid, val accepted: Deferred<Unit>)

/** The surface side of RCP: discover, open a task, follow its journal, and submit text. */
interface RcpSurface {
    suspend fun connect()

    suspend fun disconnect()

    /** Suspends until the most recent connection ends and returns the exact reason. */
    suspend fun awaitDisconnect(): Throwable

    suspend fun listTasks(): List<TaskSummary>

    suspend fun listTargets(): List<TargetSummary>

    /** Safe to repeat with the same [taskId]: the coordinator converges on one task. */
    suspend fun openTask(taskId: Uuid, nodeId: Uuid, target: String)

    /** Also stops following the task, so a later [connect] does not attach it again, and drops its unsent commands. */
    suspend fun deleteTask(taskId: Uuid)

    /**
     * Replays the journal after the stored cursor, then follows live events, calling [apply]
     * for each event in sequence order. Returns when replay is complete. The cursor moves only
     * after [apply] returns, so an event whose cursor could not be stored is delivered again on
     * the next connection: [apply] must tolerate a repeat, which [TaskEvent.eventId] identifies.
     * [connect] attaches the task again after a disconnect; if the coordinator then reports it
     * gone, that connect fails with `not_found` and the task is no longer followed.
     */
    suspend fun attach(taskId: Uuid, apply: suspend (TaskEvent) -> Unit)

    suspend fun submitText(taskId: Uuid, text: String): CommandSubmission

    /** Resends every pending command with its original identity and returns the ones admitted. */
    suspend fun retryPendingCommands(): List<Uuid>

    fun close()
}

/** State that lives only as long as the process. Tests use it; an app that must survive restarts brings its own. */
class InMemoryRcpState : RcpState {
    private val lock = Any()
    private val cursors = mutableMapOf<Uuid, Long>()
    private val pending = LinkedHashMap<Uuid, PendingTextCommand>()

    override suspend fun cursor(taskId: Uuid): Long? = synchronized(lock) { cursors[taskId] }

    override suspend fun advanceCursor(taskId: Uuid, sequence: Long) {
        synchronized(lock) { cursors[taskId] = sequence }
    }

    override suspend fun enqueueCommand(command: PendingTextCommand) {
        synchronized(lock) { pending[command.commandId] = command }
    }

    override suspend fun pendingCommands(): List<PendingTextCommand> = synchronized(lock) { pending.values.toList() }

    override suspend fun removeCommand(commandId: Uuid) {
        synchronized(lock) { pending.remove(commandId) }
    }
}

package live.renoa.app.agents

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.Json
import live.renoa.rcp.PendingTextCommand
import live.renoa.rcp.RcpState
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import kotlin.uuid.Uuid

/**
 * The outbox on this phone's disk, so a message sent just before the app was closed or killed still
 * goes after the next launch, under the identity it was first sent with. The cursors stay in memory:
 * the chat is rebuilt from a full replay on every launch, so they live only as long as it does.
 *
 * Every change rewrites [file] whole: written to a sibling, synced, then renamed over it, so an app
 * killed at any point leaves the old outbox or the new one. A file whose contents cannot be read
 * counts as an empty outbox. One that cannot be read at all is left as it is, and new messages are
 * refused until it can be.
 *
 * A change and its write finish together even when the caller is cancelled, so the outbox in memory
 * and the one on disk never part.
 */
internal class SavedOutbox(private val file: File) : RcpState {
    @Serializable
    private data class Saved(val commandId: String, val taskId: String, val text: String)

    private val next = File(file.parentFile, file.name + ".new")
    private val lock = Any()
    private val cursors = HashMap<Uuid, Long>()

    /** Held while the outbox is read or changed, so the disk sees the changes in the order they were made. */
    private val changing = Mutex()
    private var pending: LinkedHashMap<Uuid, PendingTextCommand>? = null

    override suspend fun cursor(taskId: Uuid): Long? = synchronized(lock) { cursors[taskId] }

    override suspend fun advanceCursor(taskId: Uuid, sequence: Long) {
        synchronized(lock) { cursors[taskId] = sequence }
    }

    /** Returns once the command is written and synced. One that cannot be kept is refused, so it is never sent unkept. */
    override suspend fun enqueueCommand(command: PendingTextCommand) = changed {
        val all =
            try {
                loaded()
            } catch (unread: IOException) {
                throw IllegalStateException(UNKEPT, unread)
            }
        all[command.commandId] = command
        try {
            write(all)
        } catch (failed: IOException) {
            all.remove(command.commandId)
            throw IllegalStateException(UNKEPT, failed)
        }
    }

    override suspend fun pendingCommands(): List<PendingTextCommand> = changed { loaded().values.toList() }

    override suspend fun removeCommand(commandId: Uuid) = changed {
        val all = loaded()
        if (all.remove(commandId) != null) {
            try {
                write(all)
            } catch (_: IOException) {
                // The next change writes this removal too. A launch before then sends the command again
                // under its own identity, as after a lost acknowledgement.
            }
        }
    }

    /** Forgets every unsent message, on disk too. A write already under way finishes first, so it cannot bring them back. */
    suspend fun forget() = changed {
        pending = LinkedHashMap()
        next.delete()
        file.delete()
    }

    private suspend fun <T> changed(block: () -> T): T = changing.withLock { withContext(NonCancellable + Dispatchers.IO) { block() } }

    private fun loaded(): LinkedHashMap<Uuid, PendingTextCommand> = pending ?: read().also { pending = it }

    private fun read(): LinkedHashMap<Uuid, PendingTextCommand> {
        if (!file.exists()) return LinkedHashMap()
        val text = file.readText()
        return try {
            Json.decodeFromString(ListSerializer(Saved.serializer()), text)
                .map { PendingTextCommand(Uuid.parse(it.commandId), Uuid.parse(it.taskId), it.text) }
                .associateByTo(LinkedHashMap()) { it.commandId }
        } catch (_: IllegalArgumentException) {
            LinkedHashMap()
        }
    }

    private fun write(all: Map<Uuid, PendingTextCommand>) {
        val saved = all.values.map { Saved(it.commandId.toString(), it.taskId.toString(), it.text) }
        FileOutputStream(next).use { out ->
            out.write(Json.encodeToString(ListSerializer(Saved.serializer()), saved).toByteArray(Charsets.UTF_8))
            out.fd.sync()
        }
        if (!next.renameTo(file)) throw IOException("could not replace $file")
    }

    private companion object {
        const val UNKEPT = "the message could not be kept on this phone"
    }
}

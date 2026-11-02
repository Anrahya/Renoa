package live.renoa.app.push

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.SerializationException
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import live.renoa.app.agents.replaceAtomically
import live.renoa.app.agents.replacement
import live.renoa.rcp.TaskSummary
import java.io.File
import java.io.IOException
import kotlin.uuid.Uuid

/**
 * How far this phone has told its owner about each task: the last sequence a wake-up read, so the
 * next one attaches only to the tasks that moved past it and reads only what follows. Nothing is
 * kept until the first [baseline], which records where every task stood, so a newly signed-in phone
 * is not told its whole history; a task first seen after that is read from its start.
 *
 * Kept per sign-in, as [owner] names it, in the app's private storage. Every change rewrites [file]
 * whole, as [replaceAtomically] does. A file that cannot be read, or one kept for another owner,
 * counts as nothing kept. A change that cannot be written is dropped: the next wake-up reads those
 * records again and shows them under the same notification.
 */
internal class WakeCursors(private val file: File, val owner: String) {
    @Serializable
    private data class Saved(val owner: String, val cursors: Map<String, Long>)

    private val changing = Mutex()

    /** What the file holds, once read; null inside means nothing is kept yet. */
    private var loaded: Loaded? = null

    private class Loaded(val cursors: Map<Uuid, Long>?)

    /** Whether a [baseline] was taken for this sign-in. */
    suspend fun baselined(): Boolean = changed { load() != null }

    /** The last sequence of [taskId] the owner was told about, or null for a task never read. */
    suspend fun cursor(taskId: Uuid): Long? = changed { load()?.get(taskId) }

    /** Records where each of [tasks] stands, unless a baseline was already taken. */
    suspend fun baseline(tasks: List<TaskSummary>) = changed {
        if (load() == null) save(tasks.mapNotNull { task -> task.throughSequence?.let { task.taskId to it } }.toMap())
    }

    /** Moves [taskId]'s cursor up to [sequence]; one already past it stays. */
    suspend fun advance(taskId: Uuid, sequence: Long) = changed {
        val all = load().orEmpty()
        if ((all[taskId] ?: -1) < sequence) save(all + (taskId to sequence))
    }

    /** Forgets everything kept, as on signing out. */
    suspend fun forget() = changed {
        loaded = Loaded(null)
        file.replacement.delete()
        file.delete()
    }

    private suspend fun <T> changed(block: () -> T): T = changing.withLock { withContext(NonCancellable + Dispatchers.IO) { block() } }

    private fun load(): Map<Uuid, Long>? = (loaded ?: Loaded(read()).also { loaded = it }).cursors

    private fun read(): Map<Uuid, Long>? =
        try {
            if (!file.exists()) null
            else Json.decodeFromString(Saved.serializer(), file.readText()).takeIf { it.owner == owner }?.cursors?.mapKeys { Uuid.parse(it.key) }
        } catch (_: IOException) {
            null
        } catch (_: SerializationException) {
            null
        } catch (_: IllegalArgumentException) {
            null
        }

    private fun save(cursors: Map<Uuid, Long>) {
        try {
            val saved = Saved(owner, cursors.mapKeys { it.key.toString() })
            file.replaceAtomically(Json.encodeToString(Saved.serializer(), saved).toByteArray(Charsets.UTF_8))
            loaded = Loaded(cursors)
        } catch (_: IOException) {
            // Dropped, as the class says: the next wake-up reads those records again.
        }
    }
}

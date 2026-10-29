package live.renoa.app.agents

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.KSerializer
import kotlinx.serialization.Serializable
import kotlinx.serialization.SerializationException
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.PrimitiveSerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.Json
import java.io.File
import java.io.IOException
import java.time.Instant

/** What Home shows of Renoa: everyone on the roster and their routines. */
@Serializable
data class HomeSnapshot(val agents: List<AgentSummary>, val routines: List<Routine>) {
    /**
     * As Home may show it before Renoa answers. What an agent is doing right now, and with whom,
     * cannot be known then, so no one is shown mid-task.
     */
    fun atRest(): HomeSnapshot =
        copy(
            agents =
                agents.map { agent ->
                    val state = if (agent.state == AgentState.Working) AgentState.Resting else agent.state
                    agent.copy(state = state, glance = agent.glance.copy(doing = null, with = null))
                },
        )
}

/**
 * The last [HomeSnapshot] this phone saw, so the next launch opens on it while the connection is
 * made. [owner] names the Renoa and the sign-in it belongs to; a snapshot kept for any other owner
 * reads as nothing kept. It holds no secret: the owner is an origin and a device id.
 *
 * Every save rewrites [file] whole, as [replaceAtomically] does, so an app killed at any point
 * leaves the old snapshot or the new one. A file that cannot be read counts as nothing kept.
 */
class SavedHome(private val file: File, private val owner: String, private val io: CoroutineDispatcher = Dispatchers.IO) {
    @Serializable
    private data class Saved(val owner: String, val home: HomeSnapshot)

    /** Held while the file is written or forgotten, so a write under way finishes before it is forgotten. */
    private val changing = Mutex()

    fun load(): HomeSnapshot? =
        try {
            if (!file.exists()) null else Json.decodeFromString(Saved.serializer(), file.readText()).takeIf { it.owner == owner }?.home
        } catch (_: IOException) {
            null
        } catch (_: SerializationException) {
            null
        } catch (_: IllegalArgumentException) {
            null
        }

    /**
     * Saves each snapshot of [shown] that differs at rest from the one before, until the caller is
     * cancelled. A snapshot that cannot be written is skipped; the next one writes over it.
     */
    suspend fun keep(shown: Flow<HomeSnapshot>) {
        shown.map { it.atRest() }.distinctUntilChanged().collect { home ->
            changed {
                try {
                    file.replaceAtomically(Json.encodeToString(Saved.serializer(), Saved(owner, home)).toByteArray(Charsets.UTF_8))
                } catch (_: IOException) {
                    // The next snapshot writes over it; until then the last one written stands.
                }
            }
        }
    }

    /** Forgets the snapshot, as on signing out. A write already under way finishes first, so it cannot bring it back. */
    suspend fun forget() {
        changed {
            file.replacement.delete()
            file.delete()
        }
    }

    private suspend fun <T> changed(block: () -> T): T = changing.withLock { withContext(NonCancellable + io) { block() } }
}

/**
 * Writes [bytes] to a sibling, syncs it, then renames it over this file, so a process killed at any
 * point leaves the old contents or the new.
 */
internal fun File.replaceAtomically(bytes: ByteArray) {
    val next = replacement
    next.outputStream().use { out ->
        out.write(bytes)
        out.fd.sync()
    }
    if (!next.renameTo(this)) throw IOException("could not replace $this")
}

/** The sibling [replaceAtomically] writes before renaming it over this file. */
internal val File.replacement: File get() = File(parentFile, "$name.new")

/** An instant as milliseconds since the epoch. */
internal object InstantMillis : KSerializer<Instant> {
    override val descriptor = PrimitiveSerialDescriptor("live.renoa.app.InstantMillis", PrimitiveKind.LONG)

    override fun serialize(encoder: Encoder, value: Instant) = encoder.encodeLong(value.toEpochMilli())

    override fun deserialize(decoder: Decoder): Instant = Instant.ofEpochMilli(decoder.decodeLong())
}

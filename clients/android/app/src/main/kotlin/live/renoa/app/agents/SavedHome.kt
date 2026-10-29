package live.renoa.app.agents

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.Json
import java.io.File
import java.io.IOException

/** What Home shows of Renoa: everyone on the roster and their routines. */
@Serializable
data class HomeSnapshot(val agents: List<AgentSummary>, val routines: List<Routine>) {
    /**
     * As Home may show it before Renoa answers. What an agent is doing right now, and with whom,
     * cannot be known then, so no one is shown mid-task; nor is a question it asked, which may
     * have been answered since.
     */
    fun atRest(): HomeSnapshot =
        copy(
            agents =
                agents.map { agent ->
                    val state = if (agent.state == AgentState.Working) AgentState.Resting else agent.state
                    agent.copy(state = state, glance = agent.glance.copy(doing = null, with = null, ask = null))
                },
        )
}

/**
 * The last [HomeSnapshot] this phone saw, so the next launch opens on it while the connection is
 * made. [owner] names the Renoa and the sign-in it belongs to; a snapshot kept for any other owner
 * reads as nothing kept. It holds the agents' names, purposes and last lines and the routines'
 * names, but no credential: the owner is an origin and a device id. It lives in the app's private
 * storage, which the app keeps out of backups.
 *
 * Every save rewrites [file] whole, as [replaceAtomically] does, so an app killed at any point
 * leaves the old snapshot or the new one. A file that cannot be read counts as nothing kept.
 */
class SavedHome(private val file: File, private val owner: String, private val io: CoroutineDispatcher = Dispatchers.IO) {
    @Serializable
    private data class Saved(val owner: String, val home: HomeSnapshot)

    /** Held while the file is written or forgotten, so a write under way finishes before it is forgotten. */
    private val changing = Mutex()

    /**
     * Reads what was kept for [owner], or null. It is read where the repository is made, on the
     * thread that asks for it: one small file, once a launch, so the first frame can show it.
     */
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
     * cancelled. A snapshot that cannot be written is skipped; the next one writes over it. One with
     * no agents is not kept: Renoa lists only the agents a node offers, so an empty roster may mean
     * no node is online rather than no one, and the last one kept says more.
     */
    suspend fun keep(shown: Flow<HomeSnapshot>) {
        shown.filter { it.agents.isNotEmpty() }.map { it.atRest() }.distinctUntilChanged().collect { home ->
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

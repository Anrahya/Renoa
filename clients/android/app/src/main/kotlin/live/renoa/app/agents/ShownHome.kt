package live.renoa.app.agents

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.first

/**
 * The roster and routines the screens see: what [home] kept from the last launch until Renoa's
 * first roster check answers ([heard]), then Renoa's own, each kept in turn.
 */
internal class ShownHome(private val home: SavedHome?) {
    private val kept = home?.load()
    val agents = MutableStateFlow(kept?.agents.orEmpty())
    val routines = MutableStateFlow(kept?.routines.orEmpty())

    /** Set once a roster check has gone through every agent Renoa listed. */
    val heard = MutableStateFlow(false)

    val known: StateFlow<Boolean> = if (kept != null) MutableStateFlow(true).asStateFlow() else heard.asStateFlow()

    /**
     * Once Renoa is [heard], shows every agent it has listed this launch, in [listed] order: as
     * [followed] has it, or, while its chat could not be opened, as it was kept. So a refused open
     * never shrinks the roster, and one Renoa has not listed this launch is not shown. Each is
     * marked main or not as [primary] says, kept ones too. With no [primary], before the Host has
     * said, each keeps the mark it is shown with, the last launch's at first.
     */
    fun show(listed: Collection<String>, followed: List<AgentSummary>, primary: ((agentId: String) -> Boolean)?) {
        if (!heard.value) return
        val marked = agents.value.associate { it.id to it.primary }
        agents.value =
            listed.mapNotNull { id ->
                val agent = followed.firstOrNull { it.id == id } ?: kept?.agents?.firstOrNull { it.id == id }
                agent?.copy(primary = primary?.invoke(id) ?: marked[id] ?: false)
            }
    }

    /** Keeps what is shown from the first answer on, until the caller is cancelled. */
    suspend fun keep() {
        val store = home ?: return
        heard.first { it }
        store.keep(combine(agents, routines, ::HomeSnapshot))
    }
}

package live.renoa.app.home

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.Briefing
import live.renoa.app.agents.Problem
import live.renoa.app.agents.RepositoryException
import live.renoa.app.agents.wordsOr

/**
 * Everyone, in the order Home lists them: whoever needs the owner first ([AgentState.homeRank]),
 * unread before read, then as the roster has them.
 */
fun homeOrder(agents: List<AgentSummary>): List<AgentSummary> =
    agents.sortedWith(compareBy<AgentSummary> { it.state.homeRank }.thenBy { !it.glance.unread })

data class HomeUiState(
    val briefing: Briefing? = null,
    val people: List<AgentSummary> = emptyList(),
    val notice: String? = null,
) {
    fun agent(id: String?): AgentSummary? = people.firstOrNull { it.id == id }

    /** Who the briefing is from, and who "Message …" at the bottom reaches. */
    val lead: AgentSummary? get() = agent(briefing?.agentId)
}

class HomeViewModel(private val repository: AgentRepository) : ViewModel() {
    private val notice = MutableStateFlow<String?>(null)

    /**
     * Whether Home has already shown its tiles dropping in. Home stays at the bottom of the back
     * stack, so this lives as long as the app does: coming back from a chat does not replay it.
     */
    var assembled: Boolean = false
        private set

    fun markAssembled() {
        assembled = true
    }

    val state: StateFlow<HomeUiState> =
        combine(repository.agents, repository.briefing, notice) { agents, briefing, notice -> HomeUiState(briefing, homeOrder(agents), notice) }
            .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), HomeUiState(repository.briefing.value, homeOrder(repository.agents.value)))

    /** Answers an agent's question right from Home; the agent picks it up from there. */
    fun answer(agentId: String, choice: String) {
        notice.value = null
        viewModelScope.launch {
            try {
                repository.answer(agentId, choice)
            } catch (refused: RepositoryException) {
                // A conflict means the question was already answered, from here or elsewhere: nothing to retry.
                if (refused.problem != Problem.Conflict) notice.value = refused.wordsOr("That answer did not go through. Try again.")
            }
        }
    }
}

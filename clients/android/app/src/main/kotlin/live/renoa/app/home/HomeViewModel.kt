package live.renoa.app.home

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.Briefing
import live.renoa.app.agents.Link
import live.renoa.app.agents.Problem
import live.renoa.app.agents.RepositoryException
import live.renoa.app.agents.Routine
import live.renoa.app.agents.wordsOr
import live.renoa.app.badge.mainAgent
import live.renoa.app.signup.ProfileLines
import java.util.UUID

/**
 * Everyone, in the order Home lists them: whoever needs the owner first ([AgentState.homeRank]),
 * unread before read, then as the roster has them.
 */
fun homeOrder(agents: List<AgentSummary>): List<AgentSummary> =
    agents.sortedWith(compareBy<AgentSummary> { it.state.homeRank }.thenBy { !it.glance.unread })

private const val MAX_GREETED_NAME = 40

/** The pages the dock opens, in the order they are swiped through. */
enum class HomePage(val title: String) { Chats("Chats"), Routines("Routines"), Upcoming("Upcoming") }

/** What Home keeps of its own while the owner is in a chat and back. */
private data class Local(
    val notice: String? = null,
    /** Routines being switched, with the state asked for, shown until Renoa answers. */
    val switching: Map<String, Boolean> = emptyMap(),
    val page: HomePage? = null,
    val shown: String? = null,
    val shownAt: Int = 0,
    val ownerName: String? = null,
    val signUp: Boolean = false,
)

data class HomeUiState(
    /** Every agent, as the roster has them; [people] is Home's order. */
    val roster: List<AgentSummary> = emptyList(),
    val briefing: Briefing? = null,
    val routines: List<Routine> = emptyList(),
    val notice: String? = null,
    val switching: Map<String, Boolean> = emptyMap(),
    /** The page open over Home, or null while Home itself shows. */
    val page: HomePage? = null,
    /** The handover the owner stepped to, and where it stood, so the card next to it shows once it is gone. */
    val shownKey: String? = null,
    val shownAt: Int = 0,
    /** The owner's name from their profile, once read. */
    val ownerName: String? = null,
    /** Renoa has made no agent for the owner yet, so sign-up comes first. */
    val signUp: Boolean = false,
    val link: Link = Link.Connected,
) {
    val people: List<AgentSummary> get() = homeOrder(roster)

    val main: AgentSummary? get() = mainAgent(roster)

    val handovers: List<Handover> get() = handovers(roster, briefing)

    /** Where the relay stands in [handovers]: the one stepped to, else whatever now stands in its place. */
    val shown: Int
        get() {
            val list = handovers
            val at = list.indexOfFirst { it.key == shownKey }
            return if (at >= 0) at else shownAt.coerceAtMost(list.lastIndex).coerceAtLeast(0)
        }

    fun agent(id: String?): AgentSummary? = roster.firstOrNull { it.id == id }

    /** Whether the routine shows as on: what the owner just asked for, else what Renoa last said. */
    fun isOn(routine: Routine): Boolean = switching[routine.id] ?: routine.on
}

class HomeViewModel(private val repository: AgentRepository) : ViewModel() {
    private val local = MutableStateFlow(Local())

    /**
     * Whether Home has already shown its tiles dropping in. It lives as long as Home's place on the
     * back stack, so coming back from a chat does not replay it.
     */
    var assembled: Boolean = false
        private set

    fun markAssembled() {
        assembled = true
    }

    val state: StateFlow<HomeUiState> =
        combine(repository.agents, repository.briefing, repository.routines, local, repository.link) { agents, briefing, routines, mine, link ->
            HomeUiState(agents, briefing, routines, mine.notice, mine.switching, mine.page, mine.shown, mine.shownAt, mine.ownerName, mine.signUp, link)
        }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), HomeUiState(repository.agents.value, repository.briefing.value, repository.routines.value, link = repository.link.value))

    init {
        viewModelScope.launch { meetTheOwner() }
    }

    /**
     * Asks Renoa whether the owner has an agent yet and what they are called, until it answers. An
     * owner with no agent is sent to sign-up; not reaching Renoa is no answer, so it never sends anyone.
     */
    private suspend fun meetTheOwner() {
        var wait = 2_000L
        while (true) {
            try {
                if (!repository.hasAgents()) {
                    local.update { it.copy(signUp = true) }
                    return
                }
                // A name too long for the greeting, as an agent might have written, is left out of it.
                val name = ProfileLines.name(repository.ownerProfile().content)?.takeIf { it.length <= MAX_GREETED_NAME }
                local.update { it.copy(ownerName = name) }
                return
            } catch (refused: RepositoryException) {
                // Too many calls waiting on the Host is a conflict too; only a definite refusal ends the asking.
                if (refused.problem == Problem.Invalid || refused.problem == Problem.NotFound) return
            }
            delay(wait)
            wait = (wait * 2).coerceAtMost(60_000L)
        }
    }

    /** Opens a page over Home, or turns to another. It stays open while the owner visits a chat from it. */
    fun open(page: HomePage) = local.update { if (it.page == page) it else it.copy(page = page, notice = if (it.page == null) null else it.notice) }

    /** Closes the page. A notice shown on it goes with it. */
    fun close() = local.update { it.copy(page = null, notice = null) }

    /** Steps the relay to the handover [key]. */
    fun show(key: String) = local.update { it.copy(shown = key, shownAt = state.value.handovers.indexOfFirst { h -> h.key == key }.coerceAtLeast(0)) }

    /** Answers an agent's question right from Home; the agent picks it up from there. */
    fun answer(agentId: String, choice: String) {
        local.update { it.copy(notice = null) }
        viewModelScope.launch {
            try {
                repository.answer(agentId, choice)
            } catch (refused: RepositoryException) {
                // A conflict means the question was already answered, from here or elsewhere: nothing to retry.
                if (refused.problem != Problem.Conflict) local.update { it.copy(notice = refused.wordsOr("That answer did not go through. Try again.")) }
            }
        }
    }

    /** Turns a routine on or off. Each tap is its own change, with its own operation id. */
    fun switch(routineId: String, on: Boolean) {
        local.update { it.copy(notice = null, switching = it.switching + (routineId to on)) }
        viewModelScope.launch {
            try {
                repository.setRoutine(routineId, on, UUID.randomUUID().toString())
            } catch (refused: RepositoryException) {
                val words =
                    when (refused.problem) {
                        Problem.Conflict -> "That routine was changed somewhere else. Check it and try again."
                        Problem.NotFound -> "That routine is gone."
                        else -> refused.wordsOr("That routine could not be changed. Try again.")
                    }
                local.update { it.copy(notice = words) }
            } finally {
                local.update { it.copy(switching = it.switching - routineId) }
            }
        }
    }
}

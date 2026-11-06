package live.renoa.app.home

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import live.renoa.app.agents.AgentApp
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.Feed
import live.renoa.app.agents.Link
import live.renoa.app.agents.OwnerMemory
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

/** The Renoa tab's pages, in the order its segments show them. */
enum class HomePage(val title: String) {
    Routines("Routines"),
    Memory("Memory"),
    Apps("Apps"),
}

/** What Home keeps of its own while the owner is in a chat and back. */
private data class Local(
    val notice: String? = null,
    /** Routines being switched, with the state asked for, shown until Renoa answers. */
    val switching: Map<String, Boolean> = emptyMap(),
    val page: HomePage? = null,
    val ownerName: String? = null,
    val signUp: Boolean = false,
    val memory: Remembered? = null,
    val apps: List<AgentApp>? = null,
    val appSwitching: Map<String, Boolean> = emptyMap(),
    val unread: Set<HomePage> = emptySet(),
)

/** What the Memory page shows: the owner's profile, which every agent reads, and the notes agents keep. */
data class Remembered(val profile: String, val notes: OwnerMemory)

data class HomeUiState(
    /** Every agent, as the roster has them. */
    val roster: List<AgentSummary> = emptyList(),
    /** The day's line and the notes agents left the owner. */
    val feed: Feed = Feed(),
    val routines: List<Routine> = emptyList(),
    val notice: String? = null,
    val switching: Map<String, Boolean> = emptyMap(),
    /** The Renoa tab's page while that tab shows, or null while the agents do. */
    val page: HomePage? = null,
    /** The owner's name from their profile, once read. */
    val ownerName: String? = null,
    /** Renoa has made no agent for the owner yet, so sign-up comes first. */
    val signUp: Boolean = false,
    val link: Link = Link.Connected,
    /** Whether Renoa has said who is here, or the phone kept what it last said. Until then an empty roster means not known yet, not no one. */
    val known: Boolean = true,
    /** The owner's profile and the notes agents keep, as read when the Memory page last opened; null until then. */
    val memory: Remembered? = null,
    /** The main agent's apps, as read when the Apps page last opened; null until then. */
    val apps: List<AgentApp>? = null,
    /** Apps being switched, with the state asked for, shown until Renoa answers. */
    val appSwitching: Map<String, Boolean> = emptyMap(),
    /** Pages whose last read failed, which offer to read again. */
    val unread: Set<HomePage> = emptySet(),
) {
    val main: AgentSummary? get() = mainAgent(roster)

    val talks: TalkList get() = talks(roster, feed)

    fun agent(id: String?): AgentSummary? = roster.firstOrNull { it.id == id }

    /** Whether the routine shows as on: what the owner just asked for, else what Renoa last said. */
    fun isOn(routine: Routine): Boolean = switching[routine.id] ?: routine.on

    /** Whether the app shows as on: what the owner just asked for, else what Renoa last said. */
    fun isOn(app: AgentApp): Boolean = appSwitching[app.connection] ?: app.on
}

class HomeViewModel(private val repository: AgentRepository) : ViewModel() {
    private val local = MutableStateFlow(Local())

    val state: StateFlow<HomeUiState> =
        combine(repository.agents, repository.feed, repository.routines, local, repository.link.combine(repository.rosterKnown, ::Pair)) { agents, feed, routines, mine, (link, known) ->
            HomeUiState(agents, feed, routines, mine.notice, mine.switching, mine.page, mine.ownerName, mine.signUp, link, known, mine.memory, mine.apps, mine.appSwitching, mine.unread)
        }.stateIn(
            viewModelScope,
            SharingStarted.WhileSubscribed(5_000),
            HomeUiState(repository.agents.value, repository.feed.value, repository.routines.value, link = repository.link.value, known = repository.rosterKnown.value),
        )

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
                val name = ProfileLines.name(repository.ownerProfile().content)
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

    /**
     * Shows the Renoa tab on [page]; opening the page already shown keeps its notice, and turning to
     * another clears it. It stays shown while the owner visits a chat from it. Memory and Apps are
     * read again each time they are turned to.
     */
    fun open(page: HomePage) {
        var turned = false
        local.update {
            turned = it.page != page
            if (turned) it.copy(page = page, notice = null) else it
        }
        if (turned) read(page)
    }

    /** The read of each page in flight; a newer one replaces it, so an older answer never lands last. */
    private val reads = HashMap<HomePage, Job>()

    /** Reads [page] again. A page whose read fails says so and offers this again. */
    fun read(page: HomePage) {
        if (page == HomePage.Routines) return
        reads[page]?.cancel()
        local.update { it.copy(unread = it.unread - page) }
        reads[page] =
            viewModelScope.launch {
                try {
                    when (page) {
                        HomePage.Routines -> Unit
                        HomePage.Memory -> Remembered(repository.ownerProfile().content, repository.memory()).let { memory -> local.update { it.copy(memory = memory) } }
                        HomePage.Apps -> {
                            // The tab shows only once Renoa said who is here, so a main agent is known by now.
                            val main = mainAgent(repository.agents.value) ?: throw RepositoryException(Problem.NotFound, "no main agent yet")
                            val apps = repository.apps(main.id)
                            // A switch's own answer is newer than this read.
                            local.update { if (it.appSwitching.isEmpty()) it.copy(apps = apps) else it }
                        }
                    }
                } catch (refused: RepositoryException) {
                    local.update { it.copy(unread = it.unread + page) }
                }
            }
    }

    /** Says the phone is still signed in because Renoa could not be told it signed out. */
    fun couldNotSignOut() = local.update { it.copy(notice = "Renoa couldn't be reached, so this phone is still signed in. Try again when you're online.") }

    /** Goes back to the agents. A notice shown on the Renoa tab goes with it. */
    fun close() = local.update { it.copy(page = null, notice = null) }

    /**
     * Puts away the note the owner opened, as the row it sat on opens its chat. One that cannot be
     * put away shows again, which says so.
     */
    fun seen(note: String) {
        viewModelScope.launch {
            try {
                repository.dismiss(note)
            } catch (unconfirmed: RepositoryException) {
                // The note is back on Home; the owner can open it again.
            }
        }
    }

    /** Marks [agentId] as read from Home, putting away its note if one is shown. */
    fun markRead(agentId: String) {
        state.value.talks.let { listOfNotNull(it.main) + it.talking + it.quiet }
            .firstOrNull { it.agent.id == agentId }?.note?.let { seen(it.id) }
        viewModelScope.launch {
            try {
                repository.markSeen(agentId)
            } catch (unconfirmed: RepositoryException) {
                // The unread mark stays on Home; the owner can mark it again.
            }
        }
    }

    /** Runs the routine that failed in [note] again; the note goes once it is asked for. */
    fun runAgain(note: String) {
        local.update { it.copy(notice = null) }
        viewModelScope.launch {
            try {
                repository.runAgain(note)
            } catch (refused: RepositoryException) {
                // Gone means it already ran again or was put away elsewhere: nothing to retry.
                if (refused.problem != Problem.NotFound) local.update { it.copy(notice = refused.wordsOr("That routine could not run again. Try again.")) }
            }
        }
    }

    /** Agents whose answer is on its way, so a second tap while it goes sends nothing. */
    private val answering = HashSet<String>()

    /** Answers an agent's question right from Home; the agent picks it up from there. */
    fun answer(agentId: String, choice: String) {
        if (!answering.add(agentId)) return
        local.update { it.copy(notice = null) }
        viewModelScope.launch {
            try {
                repository.answer(agentId, choice)
            } catch (refused: RepositoryException) {
                local.update { it.copy(notice = refused.wordsOr("That answer did not go through. Try again.")) }
            } finally {
                answering.remove(agentId)
            }
        }
    }

    /** Gives the main agent an app or takes it away. Its sign-in stays either way. */
    fun switchApp(connection: String, on: Boolean) {
        val main = mainAgent(repository.agents.value) ?: return
        local.update { it.copy(notice = null, appSwitching = it.appSwitching + (connection to on)) }
        viewModelScope.launch {
            var gone = false
            try {
                val apps = repository.setApp(main.id, connection, on)
                local.update { it.copy(apps = apps) }
            } catch (refused: RepositoryException) {
                gone = refused.problem == Problem.NotFound
                val words =
                    when (refused.problem) {
                        Problem.NotFound -> "That app is not connected any more. Ask ${main.name} to connect it again."
                        Problem.Conflict -> "That app can't be turned on from here. Ask ${main.name} to set it up again."
                        else -> refused.wordsOr("That app could not be changed. Try again.")
                    }
                local.update { it.copy(notice = words) }
            } finally {
                local.update { it.copy(appSwitching = it.appSwitching - connection) }
            }
            // A vanished app goes from the list rather than snap back.
            if (gone) read(HomePage.Apps)
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

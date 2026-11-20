package live.renoa.app.editor

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import live.renoa.app.agents.AbilityKind
import live.renoa.app.agents.AbilitySwitch
import live.renoa.app.agents.AgentProfile
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.wordsOr
import live.renoa.app.agents.Problem
import live.renoa.app.agents.AgentApp
import live.renoa.app.agents.AgentRules
import live.renoa.app.agents.CreateAgent
import live.renoa.app.agents.EditProfile
import live.renoa.app.agents.FieldProblem
import live.renoa.app.agents.Look
import live.renoa.app.agents.RepositoryException
import live.renoa.app.badge.mainAgent
import java.util.UUID

sealed interface EditorMode {
    data object Creating : EditorMode

    data class Editing(val agentId: String) : EditorMode
}

data class EditorUiState(
    val creating: Boolean,
    val loaded: Boolean = false,
    val name: String = "",
    val purpose: String = "",
    val abilities: List<AbilitySwitch> = emptyList(),
    val saving: Boolean = false,
    val notice: String? = null,
    /** Set when the agent was made or the edit was kept; the screen then leaves. */
    val finishedAgentId: String? = null,
    /** The agent being edited wears its own look, or Renoa's if it is the main agent, as on every other screen. */
    val look: Look? = null,
    val main: Boolean = false,
    /** The apps connected to Renoa and whether the agent being edited uses each; null until read, or while they cannot be. */
    val apps: List<AgentApp>? = null,
    /** App switches on their way, with the state asked for, shown until Renoa answers. */
    val appSwitching: Map<String, Boolean> = emptyMap(),
) {
    fun isOn(app: AgentApp): Boolean = appSwitching[app.connection] ?: app.on

    val nameProblem: FieldProblem? get() = AgentRules.checkName(name)
    val purposeProblem: FieldProblem? get() = AgentRules.checkPurpose(purpose)
    val canSave: Boolean get() = loaded && !saving && nameProblem == null && purposeProblem == null
}

/**
 * Makes an agent, or edits one. A new agent is sent with an id chosen here, once, so tapping
 * twice or retrying after a failure makes one agent. While editing, an ability switch takes
 * effect straight away; name and purpose are kept when the owner is done.
 */
class AgentEditorViewModel(
    private val repository: AgentRepository,
    private val mode: EditorMode,
    private val operationId: String = UUID.randomUUID().toString(),
) : ViewModel() {
    private val _state = MutableStateFlow(EditorUiState(creating = mode is EditorMode.Creating))
    val state: StateFlow<EditorUiState> = _state.asStateFlow()

    private var original: AgentProfile? = null
    private val abilityOrder = Mutex()

    init {
        viewModelScope.launch { load() }
        if (mode is EditorMode.Editing) {
            viewModelScope.launch {
                repository.agents.collect { roster -> _state.update { it.copy(look = roster.firstOrNull { a -> a.id == mode.agentId }?.look, main = mainAgent(roster)?.id == mode.agentId) } }
            }
        }
    }

    private suspend fun load() {
        try {
            when (mode) {
                EditorMode.Creating -> {
                    val starter = repository.starterAbilities()
                    _state.update { it.copy(loaded = true, abilities = starter) }
                }
                is EditorMode.Editing -> {
                    val profile = repository.profile(mode.agentId)
                    original = profile
                    _state.update { it.copy(loaded = true, name = profile.name, purpose = profile.purpose, abilities = profile.abilities) }
                    readApps(mode.agentId)
                }
            }
        } catch (refused: RepositoryException) {
            _state.update { it.copy(notice = refused.problem.ownerWords) }
        }
    }

    fun setName(text: String) = _state.update { it.copy(name = text, notice = null) }

    fun setPurpose(text: String) = _state.update { it.copy(purpose = text, notice = null) }

    fun toggle(abilityId: String) {
        val current = _state.value.abilities.firstOrNull { it.id == abilityId } ?: return
        val creating = _state.value.creating
        if (!current.canSwitch(creating)) {
            val why =
                when (current.kind) {
                    AbilityKind.Soon -> "${current.title} is not ready yet."
                    AbilityKind.Machine -> "${current.title} is chosen when an agent is made."
                    AbilityKind.Everyday -> "${current.title} cannot be changed here."
                }
            _state.update { it.copy(notice = why) }
            return
        }
        val wanted = !current.lit
        setLit(abilityId, wanted)
        if (creating) return
        val agentId = (mode as EditorMode.Editing).agentId
        viewModelScope.launch {
            abilityOrder.withLock {
                try {
                    val kept = repository.setAbility(agentId, abilityId, wanted)
                    _state.update { s -> s.copy(abilities = s.abilities.map { if (it.id == abilityId) kept else it }) }
                } catch (refused: RepositoryException) {
                    setLit(abilityId, !wanted)
                    _state.update { it.copy(notice = refused.problem.ownerWords) }
                }
            }
        }
    }

    fun save() {
        val s = _state.value
        if (!s.canSave) return
        _state.update { it.copy(saving = true, notice = null) }
        viewModelScope.launch {
            try {
                val id =
                    when (mode) {
                        EditorMode.Creating ->
                            repository.create(CreateAgent(operationId, s.name.trim(), s.purpose.trim(), s.abilities.filter { it.lit }.map { it.id }.toSet())).id
                        is EditorMode.Editing -> {
                            val before = checkNotNull(original) { "an edit was saved before the agent was loaded" }
                            if (before.name != s.name.trim() || before.purpose != s.purpose.trim()) {
                                repository.edit(mode.agentId, EditProfile(before.name, s.name.trim(), before.purpose, s.purpose.trim()))
                            }
                            mode.agentId
                        }
                    }
                _state.update { it.copy(saving = false, finishedAgentId = id) }
            } catch (refused: RepositoryException) {
                _state.update { it.copy(saving = false, notice = refused.problem.ownerWords) }
            }
        }
    }

    /** The apps card is left out while they cannot be read; the rest of the sheet still works. */
    private suspend fun readApps(agentId: String) {
        try {
            val apps = repository.apps(agentId)
            _state.update { it.copy(apps = apps) }
        } catch (unread: RepositoryException) {
            _state.update { it.copy(apps = null) }
        }
    }

    /** Gives the agent being edited an app or takes it away. Its sign-in stays either way. */
    fun switchApp(connection: String, on: Boolean) {
        val agentId = (mode as? EditorMode.Editing)?.agentId ?: return
        if (connection in _state.value.appSwitching) return
        _state.update { it.copy(notice = null, appSwitching = it.appSwitching + (connection to on)) }
        viewModelScope.launch {
            var gone = false
            try {
                val apps = repository.setApp(agentId, connection, on)
                _state.update { it.copy(apps = apps) }
            } catch (refused: RepositoryException) {
                gone = refused.problem == Problem.NotFound
                val words =
                    when (refused.problem) {
                        Problem.NotFound -> "That app is not connected any more."
                        Problem.Conflict -> "That app can't be turned on from here. Ask Renoa to set it up again."
                        else -> refused.wordsOr("That app could not be changed. Try again.")
                    }
                _state.update { it.copy(notice = words) }
            } finally {
                _state.update { it.copy(appSwitching = it.appSwitching - connection) }
            }
            // A vanished app goes from the list rather than snap back.
            if (gone) readApps(agentId)
        }
    }

    private fun setLit(abilityId: String, lit: Boolean) =
        _state.update { s -> s.copy(abilities = s.abilities.map { if (it.id == abilityId) it.copy(lit = lit) else it }) }
}

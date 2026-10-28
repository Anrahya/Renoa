package live.renoa.app.home

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import live.renoa.app.agents.AgentRepository

class HomeViewModel(repository: AgentRepository) : ViewModel() {
    private val hideAsleep = MutableStateFlow(false)

    val layout: StateFlow<HomeLayout> =
        combine(repository.agents, hideAsleep) { agents, hide -> HomeLayout.of(agents, hide) }
            .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), HomeLayout.of(repository.agents.value, false))

    /** Tidy up: put the sleeping agents away, or bring them back. */
    fun toggleAsleep() = hideAsleep.update { !it }
}

package live.renoa.app.chat

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.ChatMessage
import live.renoa.app.agents.Conversation
import live.renoa.app.agents.Problem
import live.renoa.app.agents.RepositoryException

/** What the composer's round button does right now. */
enum class ComposerAction {
    Send,

    /** The agent is working and nothing is typed: the button stops it. */
    Stop,

    /** Nothing typed and nothing to stop. */
    Idle,
}

data class ChatUiState(
    val agent: AgentSummary? = null,
    val messages: List<ChatMessage> = emptyList(),
    /** Words still arriving. Never stored, and gone when the reply is whole. */
    val streaming: String? = null,
    val draft: String = "",
    val notice: String? = null,
) {
    val working: Boolean get() = agent?.state == AgentState.Working

    val action: ComposerAction
        get() =
            when {
                draft.isNotBlank() -> ComposerAction.Send
                working -> ComposerAction.Stop
                else -> ComposerAction.Idle
            }
}

class ChatViewModel(private val repository: AgentRepository, private val agentId: String) : ViewModel() {
    private val draft = MutableStateFlow("")
    private val notice = MutableStateFlow<String?>(null)

    val state: StateFlow<ChatUiState> =
        combine(repository.agents, conversation(), draft, notice) { agents, conversation, draft, notice ->
            ChatUiState(agents.firstOrNull { it.id == agentId }, conversation.messages, conversation.streaming, draft, notice)
        }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), ChatUiState())

    /** An agent that is gone has an empty chat rather than a crash. */
    private fun conversation(): Flow<Conversation> =
        try {
            repository.conversation(agentId)
        } catch (gone: RepositoryException) {
            flowOf(Conversation())
        }

    fun onDraft(text: String) {
        draft.value = text
        notice.value = null
    }

    /** Sends the draft. The words leave the composer at once; if they are not kept, they come back. */
    fun send() {
        val text = draft.value.trim()
        if (text.isEmpty()) return
        draft.value = ""
        notice.value = null
        viewModelScope.launch {
            try {
                repository.send(agentId, text)
            } catch (refused: RepositoryException) {
                draft.update { if (it.isEmpty()) text else it }
                notice.value = refused.wordsOr("That did not go through. Try again.")
            }
        }
    }

    fun stop() {
        viewModelScope.launch {
            try {
                repository.stop(agentId)
            } catch (refused: RepositoryException) {
                notice.value = refused.wordsOr("Could not stop. Try again.")
            }
        }
    }
}

/** Not reaching Renoa has its own words; any other refusal gets the one that fits this screen. */
private fun RepositoryException.wordsOr(fallback: String): String = if (problem == Problem.Unavailable) problem.ownerWords else fallback

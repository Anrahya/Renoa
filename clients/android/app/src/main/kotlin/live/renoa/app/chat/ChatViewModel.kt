package live.renoa.app.chat

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.onStart
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.Ask
import live.renoa.app.agents.ChatMessage
import live.renoa.app.agents.Conversation
import live.renoa.app.agents.Link
import live.renoa.app.agents.OutgoingFile
import live.renoa.app.agents.Problem
import live.renoa.app.agents.RepositoryException
import live.renoa.app.agents.StreamingReply
import live.renoa.app.agents.wordsOr
import live.renoa.app.badge.mainAgent
import live.renoa.rcp.Attachment
import kotlin.uuid.Uuid

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
    /** A reply still arriving. Never stored; its whole message takes the same id. */
    val streaming: StreamingReply? = null,
    val draft: String = "",
    val notice: String? = null,
    /**
     * False until the chat has been read once, as while Renoa has not been reached; a screen opening
     * on it waits for this to tell old messages from new.
     */
    val loaded: Boolean = false,
    /** Whether the agent is the owner's main agent, who wears Renoa's own look. */
    val main: Boolean = false,
    val link: Link = Link.Connected,
    /** A send the owner started has not finished yet. */
    val sending: Boolean = false,
    /** Photos and files waiting in the composer to go with the next message. */
    val files: List<OutgoingFile> = emptyList(),
) {
    val working: Boolean get() = agent?.state == AgentState.Working

    /** Nothing has been said here and nothing is on its way, so the chat opens on its start. */
    val empty: Boolean get() = loaded && messages.isEmpty() && streaming == null && !working && !sending

    /** A question waiting for the owner, if the agent asked one. */
    val ask: Ask? get() = agent?.glance?.ask

    val action: ComposerAction
        get() =
            when {
                draft.isNotBlank() || files.isNotEmpty() -> ComposerAction.Send
                working -> ComposerAction.Stop
                else -> ComposerAction.Idle
            }
}

class ChatViewModel(private val repository: AgentRepository, private val agentId: String) : ViewModel() {
    private val draft = MutableStateFlow("")
    private val files = MutableStateFlow<List<OutgoingFile>>(emptyList())
    private val notice = MutableStateFlow<String?>(null)

    /** Sends the repository has not finished with yet. */
    private val inFlight = MutableStateFlow(0)

    val state: StateFlow<ChatUiState> =
        combine(repository.agents, conversation(), draft.combine(files, ::Pair), notice.combine(inFlight, ::Pair), repository.link) {
            agents, conversation, (draft, files), (notice, inFlight), link ->
            ChatUiState(
                agents.firstOrNull { it.id == agentId }, conversation?.messages.orEmpty(), conversation?.streaming, draft, notice, loaded = conversation != null,
                main = mainAgent(agents)?.id == agentId, link = link, sending = inFlight > 0, files = files,
            )
        }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), ChatUiState(link = repository.link.value))

    /**
     * The chat, or null until it is read, so the screen shows the agent and the composer at once.
     * An agent that is gone has an empty chat rather than a crash.
     */
    private fun conversation(): Flow<Conversation?> =
        try {
            repository.conversation(agentId).catch<Conversation?> { if (it is RepositoryException) emit(Conversation()) else throw it }.onStart { emit(null) }
        } catch (gone: RepositoryException) {
            flowOf(Conversation())
        }

    /** The owner is looking at the chat, so whatever the agent said has been read. */
    fun seen() {
        viewModelScope.launch { runCatching { repository.markSeen(agentId) } }
    }

    /** An answer on its way, so a second tap while it goes sends nothing. */
    private var answering = false

    fun answer(choice: String) {
        if (answering) return
        answering = true
        notice.value = null
        viewModelScope.launch {
            try {
                repository.answer(agentId, choice)
            } catch (refused: RepositoryException) {
                notice.value = refused.wordsOr("That answer did not go through. Try again.")
            } finally {
                answering = false
            }
        }
    }

    fun onDraft(text: String) {
        draft.value = text
        notice.value = null
    }

    /** Adds photos or files to go with the next message, up to [Attachment.MAX_PER_COMMAND]. */
    fun attach(picked: List<OutgoingFile>) {
        notice.value = null
        files.update { (it + picked).take(Attachment.MAX_PER_COMMAND) }
    }

    fun unattach(index: Int) {
        files.update { it.filterIndexed { at, _ -> at != index } }
    }

    /** A picked file could not be read, so it is not attached. */
    fun problem(words: String) {
        notice.value = words
    }

    /**
     * Sends the draft and its files. They leave the composer at once; if they are not kept, they
     * come back. While the agent waits on a question, words alone are the answer.
     */
    fun send() {
        val text = draft.value.trim()
        val sent = files.value
        if (text.isEmpty() && sent.isEmpty()) return
        draft.value = ""
        files.value = emptyList()
        notice.value = null
        inFlight.update { it + 1 }
        viewModelScope.launch {
            try {
                if (state.value.ask == null || sent.isNotEmpty()) repository.send(agentId, text, sent) else repository.answer(agentId, text)
            } catch (refused: RepositoryException) {
                draft.update { if (it.isEmpty()) text else it }
                files.update { (sent + it).take(Attachment.MAX_PER_COMMAND) }
                notice.value = refused.wordsOr("That did not go through. Try again.")
            } finally {
                inFlight.update { it - 1 }
            }
        }
    }

    /** Sends again a message that did not go ([ChatMessage.notSent]). */
    fun retry(messageId: String) {
        notice.value = null
        viewModelScope.launch {
            try {
                repository.retry(agentId, messageId)
            } catch (refused: RepositoryException) {
                notice.value = refused.wordsOr("That did not go through. Try again.")
            }
        }
    }

    /** Deletes a message that did not go. */
    fun discard(messageId: String) {
        viewModelScope.launch { runCatching { repository.discard(agentId, messageId) } }
    }

    /** A one-time link to the file an agent shared as [share]; see [AgentRepository.shareLink]. */
    suspend fun linkTo(share: Uuid): String = repository.shareLink(share)

    fun stop() {
        viewModelScope.launch {
            try {
                repository.stop(agentId)
            } catch (refused: RepositoryException) {
                // Not reaching Renoa leaves the stop unknown; asking again is safe.
                notice.value = if (refused.problem == Problem.Unavailable) "It may not have stopped. Try again." else "Could not stop. Try again."
            }
        }
    }
}

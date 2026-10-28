package live.renoa.app.agents

import live.renoa.rcp.CommandInput
import live.renoa.rcp.ExecutionEventKind
import live.renoa.rcp.ExecutionTerminal
import live.renoa.rcp.LiveActivity
import live.renoa.rcp.TaskEvent
import live.renoa.rcp.TaskEventKind
import java.util.UUID
import kotlin.uuid.Uuid

/**
 * The one chat the app keeps with an agent: a task whose identity every app of the owner derives
 * from the agent's node and target, so all of them, and a reinstall, open the same conversation.
 */
internal fun chatTaskId(nodeId: Uuid, target: String): Uuid =
    Uuid.parse(UUID.nameUUIDFromBytes("renoa app chat v1\n$nodeId\n$target".toByteArray(Charsets.UTF_8)).toString())

/**
 * One agent's chat, rebuilt from its task's journal and the live text of the turn that is running.
 * Not thread-safe: the caller serializes [apply], [live] and [markSeen].
 */
internal class ChatJournal {
    private val messages = mutableListOf<ChatMessage>()
    private val applied = HashSet<Uuid>()
    private val terminals = HashMap<Uuid, ExecutionTerminal>()

    /** How many assistant messages each command has recorded, which numbers the next one. */
    private val recorded = HashMap<Uuid, Int>()
    private var latestCommand: Uuid? = null

    /** Messages the owner sent that the coordinator has not admitted yet, by command. */
    private val unsent = LinkedHashMap<Uuid, String>()
    private var draft: Draft? = null
    private var seen: String? = null

    private class Draft(val commandId: Uuid, val index: Int, val text: String)

    /** Applies one journal record. A record applied before is ignored, so a replay is harmless. */
    fun apply(event: TaskEvent) {
        if (!applied.add(event.eventId)) return
        when (val kind = event.kind) {
            is TaskEventKind.CommandSubmitted -> {
                val input = kind.command.input as CommandInput.Text
                messages += ChatMessage(kind.command.commandId.toString(), Speaker.Owner, input.text)
                latestCommand = kind.command.commandId
                unsent.remove(kind.command.commandId)
            }
            is TaskEventKind.ExecutionRecorded ->
                when (val activity = kind.event.kind) {
                    is ExecutionEventKind.AssistantMessage -> {
                        val index = recorded[kind.commandId] ?: 0
                        recorded[kind.commandId] = index + 1
                        messages += ChatMessage(messageId(kind.commandId, index), Speaker.Agent, activity.text)
                        // The record supersedes the live text it completes.
                        if (draft?.let { it.commandId == kind.commandId && it.index <= index } == true) draft = null
                    }
                    is ExecutionEventKind.ExecutionTerminated -> {
                        terminals[kind.commandId] = activity.terminal
                        if (draft?.commandId == kind.commandId) draft = null
                    }
                    else -> {}
                }
        }
    }

    /**
     * Applies one live frame. `sequence` 0 opens a message, so a 0 that arrives while the previous
     * message's draft has text opens the next one, even before that message's record arrives.
     */
    fun live(commandId: Uuid, activity: LiveActivity) {
        if (commandId in terminals) return
        val current = draft?.takeIf { it.commandId == commandId }
        val first = recorded[commandId] ?: 0
        draft =
            when (activity) {
                is LiveActivity.TextReset -> current?.let { Draft(commandId, it.index, "") }
                is LiveActivity.TextDelta ->
                    when {
                        current == null -> Draft(commandId, first, activity.text)
                        activity.sequence == 0L && current.text.isNotEmpty() -> Draft(commandId, maxOf(current.index + 1, first), activity.text)
                        else -> Draft(commandId, maxOf(current.index, first), current.text + activity.text)
                    }
            }
    }

    /** Shows a message the owner sent until its record arrives; it keeps the record's id, so nothing jumps. */
    fun sending(commandId: Uuid, text: String) {
        if (messages.none { it.id == commandId.toString() }) unsent[commandId] = text
    }

    /** The coordinator refused the message, so it will never arrive. */
    fun refused(commandId: Uuid) {
        unsent.remove(commandId)
    }

    val conversation: Conversation
        get() =
            Conversation(
                messages + unsent.map { (commandId, text) -> ChatMessage(commandId.toString(), Speaker.Owner, text) },
                draft?.takeIf { it.text.isNotEmpty() }?.let { StreamingReply(messageId(it.commandId, it.index), it.text) },
            )

    /** The state the task's journal defines: working until the latest command ends, stuck if it failed. */
    val state: AgentState
        get() {
            val latest = latestCommand ?: return AgentState.Resting
            return when (terminals[latest]) {
                null -> AgentState.Working
                is ExecutionTerminal.Failed -> AgentState.Stuck
                ExecutionTerminal.Completed, is ExecutionTerminal.Cancelled -> AgentState.Resting
            }
        }

    /** The command a stop would end: the latest one, while it runs. */
    val running: Uuid?
        get() = latestCommand?.takeIf { it !in terminals }

    val glance: Glance
        get() {
            val said = messages.lastOrNull { it.speaker == Speaker.Agent }
            val last = messages.lastOrNull()
            return Glance(line = said?.text, unread = last?.speaker == Speaker.Agent && last.id != seen)
        }

    /** The owner has read everything so far. */
    fun markSeen() {
        seen = messages.lastOrNull()?.id
    }

    private fun messageId(commandId: Uuid, index: Int) = "$commandId-$index"
}

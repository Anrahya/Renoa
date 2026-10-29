package live.renoa.app.agents

import live.renoa.rcp.CommandEnvelope
import live.renoa.rcp.CommandInput
import live.renoa.rcp.ExecutionEvent
import live.renoa.rcp.ExecutionEventKind
import live.renoa.rcp.ExecutionTerminal
import live.renoa.rcp.LiveActivity
import live.renoa.rcp.TaskEvent
import live.renoa.rcp.TaskEventKind
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlin.uuid.Uuid

/** A task's journal as the coordinator records it, one sequence at a time. */
internal class Journal(val taskId: Uuid = Uuid.random()) {
    val events = mutableListOf<TaskEvent>()

    private fun record(kind: TaskEventKind) = TaskEvent(Uuid.random(), taskId, events.size + 1L, kind).also { events += it }

    fun submitted(commandId: Uuid, text: String) =
        record(TaskEventKind.CommandSubmitted(CommandEnvelope(commandId, Uuid.random(), "android", "agent", CommandInput.Text(text))))

    fun said(commandId: Uuid, text: String) = executed(commandId, ExecutionEventKind.AssistantMessage(text))

    fun ended(commandId: Uuid, terminal: ExecutionTerminal = ExecutionTerminal.Completed) =
        executed(commandId, ExecutionEventKind.ExecutionTerminated(terminal))

    fun executed(commandId: Uuid, kind: ExecutionEventKind) =
        record(TaskEventKind.ExecutionRecorded(commandId, ExecutionEvent(Uuid.random(), Uuid.random(), events.size + 1L, 0, kind)))
}

class ChatJournalTest {
    private val journal = Journal()
    private val chat = ChatJournal()
    private val command = Uuid.random()

    private fun record(event: TaskEvent) = chat.apply(event)

    private fun delta(sequence: Long, text: String) = chat.live(command, LiveActivity.TextDelta(sequence, text))

    @Test
    fun everyAppOpensTheSameChatWithAnAgentAndADifferentOneWithAnother() {
        val node = Uuid.random()
        assertEquals(chatTaskId(node, "nagisa"), chatTaskId(node, "nagisa"))
        assertNotEquals(chatTaskId(node, "nagisa"), chatTaskId(node, "kaede"))
        assertNotEquals(chatTaskId(node, "nagisa"), chatTaskId(Uuid.random(), "nagisa"))
        // The identity is fixed for good: a change would open every owner's chats empty. Computed
        // independently as MD5 of the text with the version 3 bits set.
        assertEquals(
            Uuid.parse("0ae02263-c173-3623-8a84-ed64ab6d1fbf"),
            chatTaskId(Uuid.parse("00000000-0000-4000-8000-000000000001"), "renoa"),
        )
    }

    @Test
    fun theJournalBecomesTheChatAndTheAgentsStateAndAReplayChangesNothing() {
        assertEquals(AgentState.Resting, chat.state)
        val events = listOf(journal.submitted(command, "hello"), journal.said(command, "hi"), journal.said(command, "how are you?"))
        events.forEach(::record)
        assertEquals(AgentState.Working, chat.state)
        assertEquals(command, chat.running)

        record(journal.ended(command))
        events.forEach(::record)
        assertEquals(listOf(Speaker.Owner to "hello", Speaker.Agent to "hi", Speaker.Agent to "how are you?"), chat.conversation.messages.map { it.speaker to it.text })
        assertEquals(listOf(command.toString(), "$command-0", "$command-1"), chat.conversation.messages.map { it.id })
        assertEquals(AgentState.Resting, chat.state)
        assertNull(chat.running)
    }

    @Test
    fun aFailedTurnLeavesTheAgentStuckAndACancelledOneLeavesItResting() {
        record(journal.submitted(command, "go"))
        record(journal.ended(command, ExecutionTerminal.Failed("boom")))
        assertEquals(AgentState.Stuck, chat.state)

        val next = Uuid.random()
        record(journal.submitted(next, "again"))
        assertEquals(AgentState.Working, chat.state, "the latest command decides")
        record(journal.ended(next, ExecutionTerminal.Cancelled("stopped")))
        assertEquals(AgentState.Resting, chat.state)
    }

    @Test
    fun liveTextDraftsEachMessageUntilItsRecordArrives() {
        record(journal.submitted(command, "hello"))
        delta(0, "Hel")
        delta(1, "lo")
        assertEquals(StreamingReply("$command-0", "Hello"), chat.conversation.streaming)

        // A zero opens the next message, even before the first one's record.
        delta(0, "And")
        assertEquals(StreamingReply("$command-1", "And"), chat.conversation.streaming)
        record(journal.said(command, "Hello"))
        assertEquals(StreamingReply("$command-1", "And"), chat.conversation.streaming, "the record of message 0 leaves message 1's draft")
        delta(1, " more")
        assertEquals("And more", chat.conversation.streaming?.text)

        record(journal.said(command, "And more"))
        assertNull(chat.conversation.streaming, "the record supersedes its draft")
        assertEquals(listOf("hello", "Hello", "And more"), chat.conversation.messages.map { it.text })
    }

    @Test
    fun aDraftAfterARecordNumbersTheNextMessage() {
        record(journal.submitted(command, "hello"))
        record(journal.said(command, "first"))
        delta(0, "sec")
        assertEquals("$command-1", chat.conversation.streaming?.id)
    }

    @Test
    fun aResetClearsTheDraftAndTheEndOfTheTurnClearsItForGood() {
        record(journal.submitted(command, "hello"))
        delta(0, "wrong")
        chat.live(command, LiveActivity.TextReset)
        assertNull(chat.conversation.streaming)
        // The node retries the abandoned message from sequence 0; it is still the same message.
        delta(0, "right")
        assertEquals(StreamingReply("$command-0", "right"), chat.conversation.streaming)

        record(journal.ended(command))
        assertNull(chat.conversation.streaming)
        delta(2, " late")
        assertNull(chat.conversation.streaming, "a frame after the end is ignored")
    }

    @Test
    fun aMissedFrameOrADropDiscardsTheDraftUntilTheNextMessageOpens() {
        record(journal.submitted(command, "hello"))
        delta(0, "one ")
        delta(2, "three")
        assertNull(chat.conversation.streaming, "frame 1 was missed")
        delta(3, "four")
        assertNull(chat.conversation.streaming)
        delta(0, "next")
        assertEquals(StreamingReply("$command-0", "next"), chat.conversation.streaming)

        chat.connectionLost()
        assertNull(chat.conversation.streaming)
        delta(1, " after")
        assertNull(chat.conversation.streaming, "text sent during the drop is gone, so the rest cannot be shown")
    }

    @Test
    fun whatTheAgentSaidIsUnreadUntilSeen() {
        record(journal.submitted(command, "hello"))
        assertFalse(chat.glance.unread, "the owner's own words are not unread")
        record(journal.said(command, "hi"))
        assertTrue(chat.glance.unread)
        assertEquals("hi", chat.glance.line)
        chat.markSeen()
        assertFalse(chat.glance.unread)
        record(journal.said(command, "still there?"))
        assertTrue(chat.glance.unread)
    }

    @Test
    fun aSentMessageShowsAtOnceAndItsRecordTakesItsPlace() {
        chat.sending(command, "hello")
        assertEquals(listOf(command.toString() to "hello"), chat.conversation.messages.map { it.id to it.text })
        assertEquals(AgentState.Resting, chat.state, "nothing runs until the command is admitted")

        record(journal.submitted(command, "hello"))
        chat.sending(command, "hello")
        assertEquals(listOf(command.toString()), chat.conversation.messages.map { it.id }, "shown once, as the record")

        val refused = Uuid.random()
        chat.sending(refused, "never")
        chat.refused(refused)
        assertEquals(1, chat.conversation.messages.size)
        chat.sending(refused, "never")
        assertEquals(1, chat.conversation.messages.size, "an older read of the outbox does not bring it back")
    }
}

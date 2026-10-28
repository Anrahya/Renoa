package live.renoa.rcp

import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import kotlin.test.assertTrue
import kotlin.uuid.Uuid

/** A command is written down before it is sent, retried with its own identity, and admitted once. */
class OutboxTest : CoordinatorTest() {
    private fun admittedCommands(taskId: Uuid) = coordinator.tasks.getValue(taskId).commands

    @Test
    fun aCommandIsInTheOutboxBeforeItsFrameLeavesAndGoneOnceAdmitted() = eventually {
        val taskId = coordinator.seedTask()
        var pendingWhenItArrived = emptyList<PendingTextCommand>()
        coordinator.onSubmitArrived = { pendingWhenItArrived = runBlocking { state.pendingCommands() } }
        val surface = client()
        surface.connect()

        val submission = surface.submitText(taskId, "buy coffee beans")
        submission.accepted.await()

        assertEquals(listOf(submission.commandId), pendingWhenItArrived.map { it.commandId })
        assertEquals("buy coffee beans", pendingWhenItArrived.single().text)
        assertEquals(emptyList(), state.pendingCommands())
        assertEquals(mapOf(submission.commandId.toString() to "buy coffee beans"), admittedCommands(taskId))
    }

    @Test
    fun aLostAcknowledgementIsRetriedWithTheSameIdentityAndAdmittedOnce() = eventually {
        val taskId = coordinator.seedTask()
        val surface = client()
        surface.connect()
        coordinator.dropNextAcknowledgement = true

        val submission = surface.submitText(taskId, "book the flight")
        assertFailsWith<Disconnected> { submission.accepted.await() }
        assertEquals(listOf(submission.commandId), state.pendingCommands().map { it.commandId }, "an unacknowledged command stays in the outbox")
        assertIs<Disconnected>(surface.awaitDisconnect())

        surface.connect()
        assertEquals(listOf(submission.commandId), surface.retryPendingCommands())

        val frames = coordinator.framesOfType("submit")
        assertEquals(2, frames.size)
        assertEquals(frames[0].getValue("command_id"), frames[1].getValue("command_id"))
        assertEquals(frames[0].getValue("input"), frames[1].getValue("input"))
        assertEquals(1, admittedCommands(taskId).size, "the retry must converge on one admitted command")
        assertEquals(1, coordinator.tasks.getValue(taskId).journal.size, "and one journal event")
        assertEquals(emptyList(), state.pendingCommands())
    }

    @Test
    fun retryingWithNothingPendingSendsNothing() = eventually {
        val surface = client()
        surface.connect()
        assertEquals(emptyList(), surface.retryPendingCommands())
        assertEquals(0, coordinator.framesOfType("submit").size)
    }

    @Test
    fun retryStopsAtTheFirstRejectionAndKeepsEverythingInOrder() = eventually {
        val known = coordinator.seedTask()
        val unknown = Uuid.random()
        val first = PendingTextCommand(Uuid.random(), unknown, "for a task that is gone")
        val second = PendingTextCommand(Uuid.random(), known, "for a task that exists")
        state.enqueueCommand(first)
        state.enqueueCommand(second)
        val surface = client()
        surface.connect()

        assertEquals(RcpErrorCode.NotFound, assertFailsWith<RcpException> { surface.retryPendingCommands() }.code)

        assertEquals(1, coordinator.framesOfType("submit").size, "the second command must wait behind the first")
        assertEquals(listOf(first, second), state.pendingCommands())
    }

    @Test
    fun reusingACommandIdentityWithOtherContentIsAConflictNotANewCommand() = eventually {
        val taskId = coordinator.seedTask()
        val commandId = Uuid.random()
        coordinator.tasks.getValue(taskId).commands[commandId.toString()] = "the first text"
        val surface = client(newCommandId = { commandId })
        surface.connect()

        val submission = surface.submitText(taskId, "different text")
        assertEquals(RcpErrorCode.Conflict, assertFailsWith<RcpException> { submission.accepted.await() }.code)
        assertEquals("the first text", admittedCommands(taskId).getValue(commandId.toString()))
    }

    @Test
    fun textTooLargeForOneMessageIsRefusedBeforeItReachesTheOutbox() = eventually {
        val taskId = coordinator.seedTask()
        val surface = client()
        surface.connect()

        val failure = assertFailsWith<IllegalArgumentException> { surface.submitText(taskId, "a".repeat(MAX_FRAME_BYTES + 1)) }
        assertTrue("Send shorter text" in failure.message.orEmpty(), failure.message)

        assertEquals(emptyList(), state.pendingCommands())
        assertEquals(0, coordinator.framesOfType("submit").size)
    }

    @Test
    fun aLargeButAllowedMessageIsAdmitted() = eventually {
        val taskId = coordinator.seedTask()
        val surface = client()
        surface.connect()
        val text = "é".repeat(400_000)
        surface.submitText(taskId, text).accepted.await()
        assertEquals(text, admittedCommands(taskId).values.single())
        assertEquals(text, coordinator.framesOfType("submit").single().getValue("input").jsonObject.getValue("text").jsonPrimitive.content)
    }

    @Test
    fun anOutboxThatCannotStoreStopsTheSubmitBeforeAnythingIsSent() = eventually {
        val taskId = coordinator.seedTask()
        val broken =
            object : RcpState by InMemoryRcpState() {
                override suspend fun enqueueCommand(command: PendingTextCommand) = error("disk is full")
            }
        val surface = client(rcpState = broken)
        surface.connect()

        assertFailsWith<IllegalStateException> { surface.submitText(taskId, "hello") }
        assertEquals(0, coordinator.framesOfType("submit").size)
    }

    @Test
    fun submittingNeedsAConnection() = eventually {
        val surface = client()
        assertFailsWith<IllegalStateException> { surface.submitText(Uuid.random(), "hello") }
        assertEquals(emptyList(), state.pendingCommands())
    }

    @Test
    fun anAdmissionForAnotherCommandEndsTheSession() = eventually {
        val taskId = coordinator.seedTask()
        val surface = client()
        surface.connect()
        coordinator.rewriteReply = { reply ->
            kotlinx.serialization.json.JsonObject(reply + ("command_id" to kotlinx.serialization.json.JsonPrimitive(Uuid.random().toString())))
        }
        val submission = surface.submitText(taskId, "hello")
        assertFailsWith<ProtocolViolation> { submission.accepted.await() }
        assertEquals(1, state.pendingCommands().size, "an admission for another command does not count")
        assertIs<ProtocolViolation>(surface.awaitDisconnect())
    }
}

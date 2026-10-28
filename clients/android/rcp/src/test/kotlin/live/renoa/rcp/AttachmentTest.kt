package live.renoa.rcp

import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import kotlin.uuid.Uuid

/** Replay then live events, the cursor, and what ends an attachment. */
class AttachmentTest : CoordinatorTest() {
    private fun submitted(text: String) =
        buildJsonObject {
            put("type", "command_submitted")
            put(
                "command",
                buildJsonObject {
                    put("commandId", Uuid.random().toString())
                    put("principalId", coordinator.principalId.toString())
                    put("surface", "android")
                    put("target", "agent:arcee")
                    put("input", buildJsonObject { put("type", "text"); put("text", text) })
                },
            )
        }

    private fun seedJournal(events: Int): Uuid {
        val taskId = coordinator.seedTask()
        repeat(events) { coordinator.append(taskId, submitted("event $it")) }
        return taskId
    }

    private class Applied {
        val sequences = mutableListOf<Long>()
        val apply: suspend (TaskEvent) -> Unit = { sequences += it.sequence }
    }

    @Test
    fun replayStartsAfterTheStoredCursorThenFollowsLiveEvents() = eventually {
        val taskId = seedJournal(5)
        state.advanceCursor(taskId, 2)
        val applied = Applied()
        val surface = client()
        surface.connect()

        surface.attach(taskId, applied.apply)
        assertEquals(listOf(3L, 4L), applied.sequences)
        assertEquals("2", coordinator.framesOfType("attach").single().getValue("after_sequence").toString())

        coordinator.publish(taskId, submitted("live"))
        awaitCursor(taskId, 5)
        assertEquals(listOf(3L, 4L, 5L), applied.sequences)
    }

    @Test
    fun withoutACursorTheWholeJournalIsReplayedAndTheRequestSaysNull() = eventually {
        val taskId = seedJournal(3)
        val applied = Applied()
        val surface = client()
        surface.connect()
        surface.attach(taskId, applied.apply)
        assertEquals(listOf(0L, 1L, 2L), applied.sequences)
        assertEquals("null", coordinator.framesOfType("attach").single().getValue("after_sequence").toString())
        assertEquals(2L, state.cursor(taskId))
    }

    @Test
    fun anEmptyJournalReturnsAtOnceAndLiveEventsStillArrive() = eventually {
        val taskId = coordinator.seedTask()
        val applied = Applied()
        val surface = client()
        surface.connect()
        surface.attach(taskId, applied.apply)
        assertEquals(emptyList(), applied.sequences)
        coordinator.publish(taskId, submitted("first"))
        awaitCursor(taskId, 0)
        assertEquals(listOf(0L), applied.sequences)
    }

    @Test
    fun aCursorAtTheEndOfTheJournalNeedsNoReplay() = eventually {
        val taskId = seedJournal(3)
        state.advanceCursor(taskId, 2)
        val applied = Applied()
        val surface = client()
        surface.connect()
        surface.attach(taskId, applied.apply)
        assertEquals(emptyList(), applied.sequences)
        coordinator.publish(taskId, submitted("next"))
        awaitCursor(taskId, 3)
        assertEquals(listOf(3L), applied.sequences)
    }

    @Test
    fun theCursorAdvancesOnlyAfterTheAppAppliedTheEvent() = eventually {
        val taskId = seedJournal(5)
        state.advanceCursor(taskId, 2)
        val surface = client()
        surface.connect()

        val failure = assertFailsWith<IllegalStateException> {
            surface.attach(taskId) { if (it.sequence == 3L) error("the app could not store event 3") }
        }
        assertEquals("the app could not store event 3", failure.message)
        assertEquals(2L, state.cursor(taskId), "an event the app did not store must not be acknowledged")
        assertIs<IllegalStateException>(surface.awaitDisconnect())

        // A new client with the same stored state is handed event 3 again.
        val applied = Applied()
        val second = client()
        second.connect()
        second.attach(taskId, applied.apply)
        assertEquals(listOf(3L, 4L), applied.sequences)
    }

    @Test
    fun aGapInTheSequenceEndsTheSessionAndLeavesTheCursor() = eventually {
        val taskId = seedJournal(3)
        val applied = Applied()
        val surface = client()
        surface.connect()
        surface.attach(taskId, applied.apply)

        coordinator.injectRaw(eventFrame(taskId, 5))
        assertIs<ProtocolViolation>(surface.awaitDisconnect())
        assertEquals(2L, state.cursor(taskId))
        assertEquals(listOf(0L, 1L, 2L), applied.sequences)
    }

    @Test
    fun aDuplicateEventIsIgnoredNotAppliedTwice() = eventually {
        val taskId = seedJournal(3)
        val applied = Applied()
        val surface = client()
        surface.connect()
        surface.attach(taskId, applied.apply)

        coordinator.injectRaw(eventFrame(taskId, 1))
        coordinator.publish(taskId, submitted("next"))
        awaitCursor(taskId, 3)
        assertEquals(listOf(0L, 1L, 2L, 3L), applied.sequences)
    }

    @Test
    fun anEventForATaskNobodyAttachedEndsTheSession() = eventually {
        val surface = client()
        surface.connect()
        coordinator.injectRaw(eventFrame(Uuid.random(), 0))
        assertIs<ProtocolViolation>(surface.awaitDisconnect())
    }

    @Test
    fun replayRequiredEndsTheConnectionWithItsExactErrorAndReconnectingCatchesUp() = eventually {
        val taskId = seedJournal(2)
        val applied = Applied()
        val surface = client()
        surface.connect()
        surface.attach(taskId, applied.apply)

        // Two events happen that this slow surface is never sent, then the coordinator tells it to start over.
        coordinator.append(taskId, submitted("missed 2"))
        coordinator.append(taskId, submitted("missed 3"))
        coordinator.injectRaw(coordinator.error(null, "replay_required", "this surface fell behind"))

        val reason = surface.awaitDisconnect()
        assertEquals(RcpErrorCode.ReplayRequired, assertIs<RcpException>(reason).code)

        surface.connect()
        assertEquals(listOf(0L, 1L, 2L, 3L), applied.sequences, "the attachment is restored from the cursor on connect")
        coordinator.publish(taskId, submitted("live again"))
        awaitCursor(taskId, 4)
        assertEquals(listOf(0L, 1L, 2L, 3L, 4L), applied.sequences)
    }

    @Test
    fun aFailedAttachLeavesNothingRegisteredSoItCanBeRetried() = eventually {
        val taskId = Uuid.random()
        val surface = client()
        surface.connect()
        assertEquals(RcpErrorCode.NotFound, assertFailsWith<RcpException> { surface.attach(taskId) {} }.code)

        coordinator.seedTask(taskId)
        val applied = Applied()
        surface.attach(taskId, applied.apply)
        assertFailsWith<IllegalStateException> { surface.attach(taskId, applied.apply) }
    }

    @Test
    fun aDeletedTaskIsNotFollowedAgainWhenTheClientReconnects() = eventually {
        val taskId = seedJournal(2)
        val surface = client()
        surface.connect()
        surface.attach(taskId, Applied().apply)
        surface.deleteTask(taskId)

        surface.disconnect()
        surface.connect()

        assertEquals(1, coordinator.framesOfType("attach").size, "connect must not ask to follow a task that was deleted")
    }

    @Test
    fun aTaskDeletedFromAnotherDeviceIsForgottenOnceReconnectingFindsItGone() = eventually {
        val taskId = seedJournal(2)
        val surface = client()
        surface.connect()
        surface.attach(taskId, Applied().apply)
        surface.disconnect()
        coordinator.tasks.remove(taskId)

        assertEquals(RcpErrorCode.NotFound, assertFailsWith<RcpException> { surface.connect() }.code)
        surface.connect()

        assertEquals(2, coordinator.framesOfType("attach").size, "the second connect must not ask for the gone task again")
        assertEquals(emptyList(), surface.listTasks())
    }

    @Test
    fun anEventTheAppAppliedIsDeliveredAgainWhenItsCursorCouldNotBeStored() = eventually {
        val taskId = seedJournal(5)
        state.advanceCursor(taskId, 2)
        var failOnce = true
        val flaky =
            object : RcpState by state {
                override suspend fun advanceCursor(taskId: Uuid, sequence: Long) {
                    if (sequence == 3L && failOnce) {
                        failOnce = false
                        error("the cursor could not be stored")
                    }
                    state.advanceCursor(taskId, sequence)
                }
            }
        val applied = Applied()
        val first = client(rcpState = flaky)
        first.connect()
        assertFailsWith<IllegalStateException> { first.attach(taskId, applied.apply) }
        assertEquals(2L, state.cursor(taskId))

        val second = client(rcpState = flaky)
        second.connect()
        second.attach(taskId, applied.apply)

        assertEquals(listOf(3L, 3L, 4L), applied.sequences, "an applied event whose cursor was lost is delivered again, so apply must tolerate a repeat")
    }

    private suspend fun awaitCursor(taskId: Uuid, sequence: Long) {
        while (state.cursor(taskId) != sequence) kotlinx.coroutines.delay(10)
    }

    private fun eventFrame(taskId: Uuid, sequence: Long): String =
        buildJsonObject {
            put("type", "task_event")
            put(
                "event",
                buildJsonObject {
                    put("eventId", Uuid.random().toString())
                    put("taskId", taskId.toString())
                    put("sequence", sequence)
                    put("kind", submitted("injected"))
                },
            )
        }.toString()
}

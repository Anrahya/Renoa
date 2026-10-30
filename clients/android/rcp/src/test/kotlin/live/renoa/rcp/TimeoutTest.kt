package live.renoa.rcp

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import java.net.SocketTimeoutException
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import kotlin.test.assertSame
import kotlin.test.assertTrue
import kotlin.time.Duration
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds
import kotlin.time.measureTime
import kotlin.uuid.Uuid

/** Every wait on Renoa ends in time, so the caller tries again instead of waiting forever. */
class TimeoutTest : CoordinatorTest() {
    /** Long enough for a loaded machine to connect to loopback, far below the 15-second ceiling. */
    private val short = RcpTimeouts(connect = 2.seconds, reply = 2.seconds)

    private fun journal(taskId: Uuid, events: Int) = repeat(events) { coordinator.append(taskId, coordinator.assistantMessage(Uuid.random(), "event $it")) }

    @Test
    fun aConnectionRenoaNeverSignsInIsGivenUpAtTheDeadlineAndTheNextOneConnects() = eventually {
        coordinator.unanswered = setOf("authenticate")
        val surface = client(timeouts = short)

        val failure = assertFailsWith<Disconnected> { surface.connect() }
        assertEquals("Renoa did not finish connecting within 2s", failure.message)

        coordinator.unanswered = emptySet()
        surface.connect()
        assertEquals(emptyList(), surface.listTasks())
    }

    @Test
    fun aRequestWhoseAnswerNeverComesFailsAndEndsTheConnection() = eventually {
        val surface = client(timeouts = short)
        surface.connect()
        coordinator.unanswered = setOf("list_targets")

        val failure = assertFailsWith<Disconnected> { surface.listTargets() }
        assertSame(failure, surface.awaitDisconnect(), "a late answer could match no request, so the connection ends")
        assertFailsWith<IllegalStateException> { surface.listTasks() }

        coordinator.unanswered = emptySet()
        surface.connect()
        assertEquals(listOf("agent:alpha", "agent:arcee"), surface.listTargets().map { it.target })
    }

    @Test
    fun aProbeRenoaDoesNotAnswerSoonEndsTheConnection() = eventually {
        val surface = client(timeouts = RcpTimeouts(probe = 300.milliseconds))
        surface.connect()
        surface.probe()

        coordinator.unanswered = setOf("list_targets")
        val failure = assertFailsWith<Disconnected> { surface.probe() }
        assertSame(failure, surface.awaitDisconnect())
    }

    @Test
    fun aReplayThatStopsShortFailsTheAttachAndTheRestFollowsNextTime() = eventually {
        val taskId = coordinator.seedTask()
        journal(taskId, 3)
        coordinator.replayOnly = 1
        val surface = client(timeouts = short)
        surface.connect()
        val applied = mutableListOf<Long>()

        assertFailsWith<Disconnected> { surface.attach(taskId) { applied += it.sequence } }
        assertEquals(listOf(0L), applied)

        coordinator.replayOnly = null
        surface.connect()
        surface.attach(taskId) { applied += it.sequence }
        assertEquals(listOf(0L, 1L, 2L), applied)
    }

    @Test
    fun aReplayThatKeepsComingMayTakeLongerThanTheDeadlineInAll() = eventually {
        val taskId = coordinator.seedTask()
        journal(taskId, 8)
        coordinator.replayEvery = 250
        val surface = client(timeouts = RcpTimeouts(reply = 1.seconds))
        surface.connect()
        val applied = mutableListOf<Long>()

        val took = measureTime { surface.attach(taskId) { applied += it.sequence } }

        assertEquals((0L..7L).toList(), applied)
        assertTrue(took > 1.seconds, "the replay took $took")
    }

    @Test
    fun framesHeldWhenAConnectionEndsAreDroppedAndTheNextConnectionWaitsForItsLoop() = eventually {
        val taskId = coordinator.seedTask()
        journal(taskId, 1)
        val surface = client(timeouts = short)
        surface.connect()
        val applied = CopyOnWriteArrayList<Long>()
        val holding = CompletableDeferred<Unit>()
        surface.attach(taskId) {
            applied += it.sequence
            if (it.sequence == 1L) holding.await()
        }
        surface.disconnect()
        journal(taskId, 3)

        // Event 1 holds the loop, so the replay stalls and the connection ends with 2 and 3 queued.
        assertFailsWith<Disconnected> { surface.connect() }
        coordinator.unanswered = setOf("authenticate")
        coroutineScope {
            val next = async { runCatching { surface.connect() } }
            delay(300)
            assertEquals(2, coordinator.framesOfType("authenticate").size, "the next connection waits for the last one's loop")
            holding.complete(Unit)
            assertIs<Disconnected>(next.await().exceptionOrNull())
        }
        assertEquals(listOf(0L, 1L), applied, "events queued when the connection ended are not applied")

        coordinator.unanswered = emptySet()
        surface.connect()
        assertEquals(listOf(0L, 1L, 2L, 3L), applied, "the next connection replays them from the cursor")
    }

    @Test
    fun aPeerThatVanishesIsFoundByTheKeepaliveThoughNothingElseIsSent() = eventually {
        coordinator.vanishAfterAuthenticating = true
        val surface = client(timeouts = RcpTimeouts(keepalive = 200.milliseconds))
        surface.connect()

        val ended = assertIs<Disconnected>(surface.awaitDisconnect())
        assertIs<SocketTimeoutException>(ended.cause, "the ping went unanswered")
    }

    @Test
    fun everyTimeoutIsPositiveAndFinite() {
        assertFailsWith<IllegalArgumentException> { RcpTimeouts(keepalive = Duration.ZERO) }
        assertFailsWith<IllegalArgumentException> { RcpTimeouts(reply = Duration.INFINITE) }
        assertFailsWith<IllegalArgumentException> { RcpTimeouts(close = (-1).seconds) }
    }
}

package live.renoa.rcp

import java.net.SocketTimeoutException
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import kotlin.test.assertSame
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds
import kotlin.uuid.Uuid

/** Every wait on Renoa ends in time, so the caller tries again instead of waiting forever. */
class TimeoutTest : CoordinatorTest() {
    /** Long enough for a loaded machine to connect to loopback, far below the 15-second ceiling. */
    private val short = RcpTimeouts(connect = 2.seconds, reply = 2.seconds)

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
    fun aReplayThatStopsShortFailsTheAttachAndTheRestFollowsNextTime() = eventually {
        val taskId = coordinator.seedTask()
        repeat(3) { coordinator.append(taskId, coordinator.assistantMessage(Uuid.random(), "event $it")) }
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
    fun aPeerThatVanishesIsFoundByTheKeepaliveThoughNothingElseIsSent() = eventually {
        coordinator.vanishAfterAuthenticating = true
        val surface = client(timeouts = RcpTimeouts(keepalive = 200.milliseconds))
        surface.connect()

        val ended = assertIs<Disconnected>(surface.awaitDisconnect())
        assertIs<SocketTimeoutException>(ended.cause, "the ping went unanswered")
    }

    @Test
    fun theAppsClientPingsAndGivesTheHostTimeToAnswer() {
        val app = RcpTimeouts()
        assertTrue(app.keepalive.isPositive(), "without a ping, a half-open socket is never found")
        assertTrue(app.hostReply > 60.seconds, "the coordinator waits up to 60 seconds for the Host before it answers")
    }
}

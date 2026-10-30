package live.renoa.app.agents

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import live.renoa.rcp.Authentication
import live.renoa.rcp.FakeCoordinator
import live.renoa.rcp.InMemoryRcpState
import live.renoa.rcp.RcpTimeouts
import live.renoa.rcp.WebSocketRcpSurface
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds

/** The repository over the real client, against a coordinator that speaks only raw JSON. */
class ReconnectTest {
    @Test
    fun aSignInRenoaNeverAnswersIsGivenUpAndRetriedUntilRenoaAnswers() = runBlocking(Dispatchers.Default) {
        FakeCoordinator().use { coordinator ->
            coordinator.unanswered = setOf("authenticate")
            val state = InMemoryRcpState()
            val surface = WebSocketRcpSurface(coordinator.endpoint, Authentication.Device(coordinator.credentials), state, timeouts = RcpTimeouts(connect = 1.seconds))
            val running = CoroutineScope(SupervisorJob() + Dispatchers.Default)
            try {
                val repo = RcpAgentRepository(surface, state, running, retryAfter = { 50.milliseconds })
                repo.start()
                withTimeout(15.seconds) {
                    repo.link.first { it == Link.Lost }
                    while (coordinator.framesOfType("authenticate").size < 2) delay(10)

                    coordinator.unanswered = emptySet()
                    repo.link.first { it == Link.Connected }
                }
                assertEquals(listOf("Alpha", "Arcee"), repo.agents.value.map { it.name }.sorted())
            } finally {
                running.cancel()
                surface.close()
            }
        }
    }

    @Test
    fun aMessageWhoseAcceptanceIsLostGoesAgainUnderItsIdentityAndIsAdmittedOnce() = runBlocking(Dispatchers.Default) {
        FakeCoordinator().use { coordinator ->
            val state = InMemoryRcpState()
            val surface = WebSocketRcpSurface(coordinator.endpoint, Authentication.Device(coordinator.credentials), state, timeouts = RcpTimeouts(reply = 1.seconds))
            val running = CoroutineScope(SupervisorJob() + Dispatchers.Default)
            try {
                val repo = RcpAgentRepository(surface, state, running, retryAfter = { 50.milliseconds })
                repo.start()
                withTimeout(15.seconds) {
                    repo.link.first { it == Link.Connected }
                    coordinator.withholdNextAcknowledgement = true
                    repo.send("${coordinator.nodeId}/agent:arcee", "hello")
                    while (state.pendingCommands().isNotEmpty()) delay(10)
                }
                val submits = coordinator.framesOfType("submit")
                assertEquals(2, submits.size, "sent, then sent again after the reconnect")
                assertEquals(1, submits.map { it.getValue("command_id") }.toSet().size, "under one identity")
                val chat = coordinator.tasks.getValue(chatTaskId(coordinator.nodeId, "agent:arcee"))
                assertEquals(listOf("hello"), chat.commands.values.toList(), "admitted once")
                assertEquals(1, chat.journal.size)
            } finally {
                running.cancel()
                surface.close()
            }
        }
    }
}

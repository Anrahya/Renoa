package live.renoa.rcp

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.uuid.Uuid

/** Connection, discovery and request handling against a coordinator that speaks only raw JSON. */
class SessionTest : CoordinatorTest() {
    @Test
    fun deviceCredentialsAuthenticateAndAnOtherwiseEmptyAccountListsNothing() = eventually {
        val surface = client()
        surface.connect()
        assertEquals(emptyList(), surface.listTasks())
        assertEquals(1, coordinator.framesOfType("authenticate").size)
    }

    @Test
    fun wrongCredentialsAreRefusedWithTheTypedError() = eventually {
        val surface = client(Authentication.Device(DeviceCredentials(coordinator.credentials.deviceId, "x".repeat(64))))
        val failure = assertFailsWith<RcpException> { surface.connect() }
        assertEquals(RcpErrorCode.AuthenticationFailed, failure.code)
        assertFailsWith<IllegalStateException> { surface.listTasks() }
    }

    @Test
    fun aTicketIsUsableOnceAndEachConnectionAsksForAnother() = eventually {
        val ticket = "ab".repeat(32)
        val second = "cd".repeat(32)
        coordinator.validTickets += setOf(ticket, second)
        val issued = ArrayDeque(listOf(ticket, ticket, second))
        val surface = client(Authentication.Ticket { issued.removeFirst() })

        surface.connect()
        surface.disconnect()
        // The coordinator deleted the first ticket, so replaying it fails.
        assertEquals(RcpErrorCode.AuthenticationFailed, assertFailsWith<RcpException> { surface.connect() }.code)
        surface.connect()
        assertEquals(emptyList(), surface.listTasks())
    }

    @Test
    fun aMalformedTicketNeverReachesTheNetwork() = eventually {
        val surface = client(Authentication.Ticket { "not-hex" })
        assertFailsWith<IllegalArgumentException> { surface.connect() }
        assertEquals(0, coordinator.connectionsAccepted.get())
    }

    @Test
    fun aCoordinatorThatRefusesTheBindingVersionEndsTheSessionWithTheTypedError() = eventually {
        coordinator.refuseVersion = true
        val surface = client()
        assertEquals(RcpErrorCode.VersionMismatch, assertFailsWith<RcpException> { surface.connect() }.code)
    }

    @Test
    fun aCoordinatorThatAnswersWithAnotherBindingVersionIsNotTrusted() = eventually {
        coordinator.authenticatedVersion = 12
        val surface = client()
        assertIs<ProtocolViolation>(assertFailsWith<ProtocolViolation> { surface.connect() })
    }

    @Test
    fun discoveryOpenAndDeleteConvergeOnOneTask() = eventually {
        val surface = client()
        surface.connect()

        val targets = surface.listTargets()
        assertEquals(listOf("agent:alpha", "agent:arcee"), targets.map { it.target })

        val taskId = Uuid.random()
        surface.openTask(taskId, coordinator.nodeId, "agent:arcee")
        surface.openTask(taskId, coordinator.nodeId, "agent:arcee")
        assertEquals(listOf(taskId), surface.listTasks().map { it.taskId })

        surface.deleteTask(taskId)
        surface.deleteTask(taskId)
        assertEquals(emptyList(), surface.listTasks())
    }

    @Test
    fun anOfflineNodeAndAnUnadvertisedTargetAreTypedErrorsAndTheSessionSurvives() = eventually {
        val surface = client()
        surface.connect()
        coordinator.nodeOnline = false
        val offline = assertFailsWith<RcpException> { surface.openTask(Uuid.random(), coordinator.nodeId, "agent:arcee") }
        assertEquals(RcpErrorCode.NodeOffline, offline.code)
        coordinator.nodeOnline = true
        val unknown = assertFailsWith<RcpException> { surface.openTask(Uuid.random(), coordinator.nodeId, "agent:nobody") }
        assertEquals(RcpErrorCode.NotFound, unknown.code)
        assertEquals(emptyList(), surface.listTasks())
    }

    @Test
    fun aReplyForAnotherTaskEndsTheSession() = eventually {
        val surface = client()
        surface.connect()
        val other = Uuid.random().toString()
        coordinator.rewriteReply = { JsonObject(it + ("task_id" to JsonPrimitive(other))) }
        assertFailsWith<ProtocolViolation> { surface.openTask(Uuid.random(), coordinator.nodeId, "agent:arcee") }
        assertIs<ProtocolViolation>(surface.awaitDisconnect())
    }

    @Test
    fun aReplyNobodyAskedForEndsTheSession() = eventually {
        val surface = client()
        surface.connect()
        coordinator.injectRaw("""{"type":"task_deleted","request_id":99,"task_id":"${Uuid.random()}"}""")
        assertIs<ProtocolViolation>(surface.awaitDisconnect())
        assertFailsWith<IllegalStateException> { surface.listTasks() }
    }

    @Test
    fun aReplyOfTheWrongKindEndsTheSession() = eventually {
        val surface = client()
        surface.connect()
        coordinator.rewriteReply = { JsonObject(it + ("type" to JsonPrimitive("target_list")) + ("targets" to JsonArray(emptyList()))) }
        assertFailsWith<ProtocolViolation> { surface.listTasks() }
        assertIs<ProtocolViolation>(surface.awaitDisconnect())
    }

    @Test
    fun plainTextEndpointsAreRefusedExceptOnLoopback() {
        val credentials = Authentication.Device(coordinator.credentials)
        assertFailsWith<IllegalArgumentException> { WebSocketRcpSurface("ws://renoa.live/connect", credentials, state) }
        WebSocketRcpSurface("wss://renoa.live/connect", credentials, state).close()
        WebSocketRcpSurface("ws://localhost:9/connect", credentials, state).close()
        WebSocketRcpSurface("ws://[::1]:9/connect", credentials, state).close()
    }

    @Test
    fun aClosedClientCannotReconnect() = eventually {
        val surface = client()
        surface.connect()
        surface.close()
        assertFailsWith<IllegalStateException> { surface.connect() }
    }

    @Test
    fun aSecondConnectWhileConnectedIsRefused() = eventually {
        val surface = client()
        surface.connect()
        assertFailsWith<IllegalStateException> { surface.connect() }
    }

    @Test
    fun aRedirectIsNeverFollowedSoCredentialsGoOnlyWhereTheEndpointSaid() = eventually {
        FakeCoordinator().use { elsewhere ->
            coordinator.redirectTo = elsewhere.endpoint.replaceFirst("ws://", "http://")
            val surface = client()

            assertFailsWith<Disconnected> { surface.connect() }

            assertEquals(0, elsewhere.connectionsAccepted.get(), "the redirect target must never be contacted")
            assertEquals(emptyList(), elsewhere.received)
        }
    }

    @Test
    fun closingWhileATicketIsStillBeingFetchedEndsTheConnectCall() = eventually {
        val ticket = "ab".repeat(32)
        coordinator.validTickets += ticket
        val asked = CompletableDeferred<Unit>()
        val issued = CompletableDeferred<String>()
        val surface = client(Authentication.Ticket { asked.complete(Unit); issued.await() })
        coroutineScope {
            val connecting = async { runCatching { surface.connect() } }
            asked.await()

            surface.close()
            issued.complete(ticket)

            assertIs<Disconnected>(connecting.await().exceptionOrNull())
        }
    }

    @Test
    fun aSecretIsNeverPrintedWithItsHolder() {
        val credential = "s3cret".repeat(10)
        val ticket = "ab".repeat(32)
        val credentials = DeviceCredentials(Uuid.random(), credential)
        val printed =
            listOf(
                credentials,
                Authentication.Device(credentials),
                ClientMessage.Authenticate(RCP_BINDING_VERSION, credentials),
                ClientMessage.AuthenticateTicket(RCP_BINDING_VERSION, ticket),
            ).joinToString()
        assertFalse(credential in printed, printed)
        assertFalse(ticket in printed, printed)
    }
}

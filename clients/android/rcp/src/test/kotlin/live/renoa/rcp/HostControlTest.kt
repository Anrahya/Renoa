package live.renoa.rcp

import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.jsonPrimitive
import java.io.File
import java.security.KeyPairGenerator
import java.security.Signature
import java.security.spec.ECGenParameterSpec
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNotNull
import kotlin.test.assertTrue
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds
import kotlin.uuid.Uuid

/** A signed-in app calls its owner's Host over its connection. */
class HostControlTest : CoordinatorTest() {
    private suspend fun keyedClient(timeouts: RcpTimeouts = RcpTimeouts()): RcpSurface {
        val key = KeyPairGenerator.getInstance("EC").run {
            initialize(ECGenParameterSpec("secp256r1"))
            generateKeyPair()
        }
        coordinator.deviceKey = key.public
        val sign: suspend (ByteArray) -> ByteArray = { bytes ->
            Signature.getInstance("SHA256withECDSA").run {
                initSign(key.private)
                update(bytes)
                sign()
            }
        }
        return client(Authentication.Device(coordinator.credentials, sign), timeouts = timeouts).also { it.connect() }
    }

    @Test
    fun aHostCallWaitsLongerThanOtherRequestsButNotForever() = eventually {
        coordinator.host = { _, _, _ ->
            Thread.sleep(600)
            200 to "{}"
        }
        val app = keyedClient(RcpTimeouts(reply = 200.milliseconds, hostReply = 2.seconds))
        assertEquals(HostAnswer(200, "{}"), app.callHost(HostMethod.Get, "/v1/host", null), "a slow Host is answered past the reply deadline")

        coordinator.unanswered = setOf("call_host")
        val failure = assertFailsWith<Disconnected> { app.callHost(HostMethod.Get, "/v1/host", null) }
        assertTrue(failure.message.orEmpty().startsWith("Renoa sent nothing for 2s"), failure.message)
    }

    @Test
    fun theAppWaitsLongerForAHostCallThanTheCoordinatorDoes() {
        val source = File(System.getProperty("renoa.docs"), "../crates/renoa-control/src/host_calls.rs").readText()
        val coordinator = Regex("""HOST_CALL_TIMEOUT: Duration = Duration::from_secs\((\d+)\)""").find(source)
        assertNotNull(coordinator, "renoa-control no longer declares HOST_CALL_TIMEOUT in seconds; read it again")
        val waits = coordinator.groupValues[1].toLong().seconds
        assertTrue(RcpTimeouts().hostReply > waits, "the coordinator answers a Host call after up to $waits")
    }

    @Test
    fun aCallReachesTheHostAsSentAndItsAnswerReturnsEvenARefusal() = eventually {
        coordinator.host = { method, path, body -> if (method == "POST") 201 to """{"path":"$path","body":$body}""" else 404 to """{"code":"not_found"}""" }
        val app = keyedClient()

        assertEquals(HostAnswer(201, """{"path":"/v1/host/agents","body":{"name":"Tifa"}}"""), app.callHost(HostMethod.Post, "/v1/host/agents", """{"name":"Tifa"}"""))
        assertEquals(HostAnswer(404, """{"code":"not_found"}"""), app.callHost(HostMethod.Get, "/v1/host/agents/x", null), "the Host's refusal is an answer")

        val read = coordinator.framesOfType("call_host").last()
        assertEquals("GET", read.getValue("method").jsonPrimitive.content)
        assertEquals(JsonNull, read.getValue("body"), "a read sends body null")
    }

    @Test
    fun anOfflineHostOrAConnectionWithoutAKeyIsAnRcpError() = eventually {
        coordinator.host = { _, _, _ -> null }
        val app = keyedClient()
        assertEquals(RcpErrorCode.NodeOffline, assertFailsWith<RcpException> { app.callHost(HostMethod.Get, "/v1/host", null) }.code)

        coordinator.deviceKey = null
        val bearer = client().also { it.connect() }
        assertEquals(RcpErrorCode.InvalidRole, assertFailsWith<RcpException> { bearer.callHost(HostMethod.Get, "/v1/host", null) }.code)
    }

    @Test
    fun aSignedInAppGetsAFreshLinkToEachShareAndNoOtherConnectionDoes() = eventually {
        val share = Uuid.parse("0f0e0d0c-0b0a-4908-8706-050403020100")
        val app = keyedClient()
        val first = app.shareLink(share)
        val second = app.shareLink(share)
        assertTrue(first.startsWith("/s/$share?ticket="), "the link names the share: $first")
        assertTrue(first != second, "each link is new")
        assertEquals(listOf(share.toString(), share.toString()), coordinator.sharedLinks)

        coordinator.deviceKey = null
        val bearer = client().also { it.connect() }
        assertEquals(RcpErrorCode.InvalidRole, assertFailsWith<RcpException> { bearer.shareLink(share) }.code)
    }

    @Test
    fun aSignedInAppApprovesTheComputerShowingACodeAndNoOtherConnectionDoes() = eventually {
        coordinator.waitingComputers["7K3MQX9D"] = "MacBook"
        val app = keyedClient()
        assertEquals(ApprovedDevice("mac", "MacBook"), app.approveDevice("7K3m-qx9D"))
        assertEquals(listOf("7K3m-qx9D"), coordinator.approvedCodes, "the code is sent as typed")
        assertEquals(RcpErrorCode.NotFound, assertFailsWith<RcpException> { app.approveDevice("ZZZZZZZZ") }.code)

        coordinator.deviceKey = null
        val bearer = client().also { it.connect() }
        assertEquals(RcpErrorCode.InvalidRole, assertFailsWith<RcpException> { bearer.approveDevice("7K3MQX9D") }.code)
        assertEquals(1, coordinator.approvedCodes.size)
    }

    @Test
    fun aComputerCodeNeverReachesALog() {
        val frame = ClientMessage.ApproveDevice(7, "7K3MQX9D")
        assertTrue("7K3MQX9D" !in frame.toString(), frame.toString())
    }

    @Test
    fun aShareLinkNeverReachesALog() {
        val frame = ServerMessage.ShareLinkIssued(7, "/s/x?ticket=${"ab".repeat(32)}")
        assertTrue("ab".repeat(32) !in frame.toString(), frame.toString())
    }
}

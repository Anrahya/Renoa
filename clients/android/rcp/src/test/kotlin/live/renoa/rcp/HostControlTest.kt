package live.renoa.rcp

import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.jsonPrimitive
import java.security.KeyPairGenerator
import java.security.Signature
import java.security.spec.ECGenParameterSpec
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

/** A signed-in app calls its owner's Host over its connection. */
class HostControlTest : CoordinatorTest() {
    private suspend fun keyedClient(): RcpSurface {
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
        return client(Authentication.Device(coordinator.credentials, sign)).also { it.connect() }
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
}

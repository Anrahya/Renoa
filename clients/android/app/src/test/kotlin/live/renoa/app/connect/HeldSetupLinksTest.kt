package live.renoa.app.connect

import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import live.renoa.rcp.Authentication
import live.renoa.rcp.FakeCoordinator
import live.renoa.rcp.HeldNotice
import live.renoa.rcp.InMemoryRcpState
import live.renoa.rcp.NoticeKey
import live.renoa.rcp.WebSocketRcpSurface
import java.io.File
import java.math.BigInteger
import java.security.KeyFactory
import java.security.interfaces.ECPublicKey
import java.security.spec.ECPrivateKeySpec
import java.security.spec.X509EncodedKeySpec
import java.util.Base64
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.uuid.Uuid

/** The phone reads its setup links from Renoa over a real socket, opening each with its own key. */
class HeldSetupLinksTest {
    private val coordinator = FakeCoordinator()
    private val surface = WebSocketRcpSurface(coordinator.endpoint, Authentication.Device(coordinator.credentials), InMemoryRcpState())

    /** The node's fixed vector: a notice sealed to a known key for a known device. */
    private val vector =
        Json.parseToJsonElement(File(System.getProperty("renoa.docs"), "../crates/renoa-node/tests/setup-notice-vector.json").readText()).jsonObject

    private fun text(name: String) = vector.getValue(name).jsonPrimitive.content

    private fun bytes(name: String) = Base64.getUrlDecoder().decode(text(name))

    private val deviceId = Uuid.parse(text("device_id"))
    private val sealed = HeldNotice(Uuid.parse(text("notice_id")), text("sealed"), 1_893_456_000_000)
    private val garbled = HeldNotice(Uuid.random(), "AQ", 1_893_456_000_000)

    private val key: NoticeKey =
        run {
            val point = bytes("public_key")
            // The vector's point, as an encoded key, lends its curve parameters to the private half.
            val params = (KeyFactory.getInstance("EC").generatePublic(X509EncodedKeySpec(x509(point))) as ECPublicKey).params
            val private = KeyFactory.getInstance("EC").generatePrivate(ECPrivateKeySpec(BigInteger(1, bytes("private_key")), params))
            NoticeKey.decode(point + private.encoded)
        }

    private fun links(now: Long = 0) = HeldSetupLinks(surface, deviceId, { key }, now = { now })

    @AfterTest
    fun tearDown() {
        surface.close()
        coordinator.close()
    }

    @Test
    fun aConnectionRegistersTheKeyAndShowsWhatOpens() =
        runBlocking {
            surface.connect()
            coordinator.notices += listOf(sealed, garbled)
            val held = links()
            held.connected()

            assertEquals(Base64.getUrlEncoder().withoutPadding().encodeToString(key.publicKey), coordinator.noticeKey)
            assertEquals(listOf(sealed.noticeId), held.links.value.map { it.noticeId })
            assertEquals("Notion MCP", held.links.value.single().link.label)
            assertEquals(listOf(sealed), coordinator.notices.toList(), "a notice that cannot open here is acknowledged away")
        }

    @Test
    fun anExpiredLinkIsNotShown() =
        runBlocking {
            surface.connect()
            coordinator.notices += sealed
            val held = links(now = 1_893_456_000_000)
            held.read()
            assertEquals(emptyList(), held.links.value)
        }

    @Test
    fun aLinkActedOnIsGoneEverywhere() =
        runBlocking {
            surface.connect()
            coordinator.notices += sealed
            val held = links()
            held.read()
            held.done(sealed.noticeId)
            assertEquals(emptyList(), held.links.value)
            assertEquals(emptyList(), coordinator.notices.toList())
        }

    /** [point] as an X.509 SubjectPublicKeyInfo: the fixed P-256 prefix, then the point. */
    private fun x509(point: ByteArray): ByteArray =
        Base64.getDecoder().decode("MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgA=") + point
}

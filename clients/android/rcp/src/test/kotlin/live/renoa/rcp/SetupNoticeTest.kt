package live.renoa.rcp

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import java.io.File
import java.math.BigInteger
import java.security.KeyFactory
import java.security.spec.ECPrivateKeySpec
import java.util.Base64
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.uuid.Uuid

/** A setup notice opens on its device exactly as the node sealed it, and as nothing else. */
class SetupNoticeTest : CoordinatorTest() {
    /** The node's fixed vector, which `crates/renoa-node` checks its sealing against. */
    private val vector =
        Json.parseToJsonElement(File(System.getProperty("renoa.docs"), "../crates/renoa-node/tests/setup-notice-vector.json").readText()).jsonObject

    private fun text(name: String) = vector.getValue(name).jsonPrimitive.content

    private fun bytes(name: String) = Base64.getUrlDecoder().decode(text(name))

    private val deviceId = Uuid.parse(text("device_id"))
    private val notice = HeldNotice(Uuid.parse(text("notice_id")), text("sealed"), 1_893_456_000_000)

    private fun vectorKey(): NoticeKey {
        val point = bytes("public_key")
        val private = KeyFactory.getInstance("EC").generatePrivate(ECPrivateKeySpec(BigInteger(1, bytes("private_key")), p256PublicKey(point).params))
        return NoticeKey.decode(point + private.encoded)
    }

    @Test
    fun theNodesVectorOpensToItsLink() {
        val link = vectorKey().open(notice, deviceId)
        val plaintext = Json.parseToJsonElement(text("plaintext")).jsonObject
        assertEquals(SetupLinkKind.Authorization, link.kind)
        assertEquals(plaintext.getValue("label").jsonPrimitive.content, link.label)
        assertEquals(plaintext.getValue("url").jsonPrimitive.content, link.url)
        assertEquals(plaintext.getValue("expires_at_ms").jsonPrimitive.long, link.expiresAtMs)
    }

    @Test
    fun aNoticeOpensOnlyAsItselfOnItsDeviceWithItsKey() {
        val key = vectorKey()
        assertFailsWith<UnreadableNotice> { key.open(notice.copy(noticeId = Uuid.random()), deviceId) }
        assertFailsWith<UnreadableNotice> { key.open(notice, Uuid.random()) }
        assertFailsWith<UnreadableNotice> { NoticeKey.generate().open(notice, deviceId) }
        val sealed = bytes("sealed").also { it[it.size - 1] = (it.last() + 1).toByte() }
        assertFailsWith<UnreadableNotice> { key.open(notice.copy(sealed = Base64.getUrlEncoder().withoutPadding().encodeToString(sealed)), deviceId) }
    }

    @Test
    fun aKeptKeyIsTheSameKey() {
        val key = NoticeKey.generate()
        val kept = NoticeKey.decode(key.encoded())
        assertContentEquals(key.publicKey, kept.publicKey)
        assertEquals(65, key.publicKey.size)
    }

    @Test
    fun aDeviceRegistersItsKeyListsAndAcknowledges() = eventually {
        val app = client().also { it.connect() }
        val key = NoticeKey.generate()
        app.setNoticeKey(key.publicKey)
        assertEquals(Base64.getUrlEncoder().withoutPadding().encodeToString(key.publicKey), coordinator.noticeKey)

        coordinator.notices += notice
        assertEquals(listOf(notice), app.listNotices())
        app.acknowledgeNotice(notice.noticeId)
        app.acknowledgeNotice(notice.noticeId)
        assertEquals(emptyList(), app.listNotices())
    }

    @Test
    fun aTicketConnectionIsRefused() = eventually {
        val ticket = "cd".repeat(32)
        coordinator.validTickets += ticket
        val browser = client(Authentication.Ticket { ticket }).also { it.connect() }
        assertEquals(RcpErrorCode.InvalidRole, assertFailsWith<RcpException> { browser.setNoticeKey(NoticeKey.generate().publicKey) }.code)
        assertEquals(RcpErrorCode.InvalidRole, assertFailsWith<RcpException> { browser.listNotices() }.code)
    }
}

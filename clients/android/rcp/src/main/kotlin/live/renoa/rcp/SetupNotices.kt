package live.renoa.rcp

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import java.math.BigInteger
import java.net.URI
import java.security.AlgorithmParameters
import java.security.KeyFactory
import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.security.spec.ECParameterSpec
import java.security.spec.ECPoint
import java.security.spec.ECPublicKeySpec
import java.security.spec.PKCS8EncodedKeySpec
import java.util.Base64
import javax.crypto.AEADBadTagException
import javax.crypto.Cipher
import javax.crypto.KeyAgreement
import javax.crypto.Mac
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec
import kotlin.uuid.Uuid

/** Names the envelope wherever its bytes are bound, as the binding's setup notices section sets it. */
private val DOMAIN = "renoa.setup-notice.v1".toByteArray(Charsets.US_ASCII)
private const val VERSION: Byte = 1
private const val POINT_BYTES = 65
private const val NONCE_BYTES = 12
private const val TAG_BYTES = 16

/** What a setup link connects: a provider's sign-in, or Renoa's encrypted credential page. */
@Serializable
enum class SetupLinkKind {
    @SerialName("authorization")
    Authorization,

    @SerialName("credentials")
    Credentials,
}

/** A setup link a node sealed to this device. [label] names what is being connected; [url] is https. */
@Serializable
data class SetupLink(
    val kind: SetupLinkKind,
    val label: String,
    val url: String,
    @SerialName("expires_at_ms") val expiresAtMs: Long,
) {
    /** The link carries a key in its fragment for the credential page: printing the record must not print it. */
    override fun toString(): String = "SetupLink(kind=$kind, label=$label, url=<secret>, expiresAtMs=$expiresAtMs)"
}

/** A notice that does not open on this device as itself, or opens to something that is not a setup link. */
class UnreadableNotice(message: String, cause: Throwable? = null) : Exception(message, cause)

/**
 * This device's P-256 key for setup notices. [publicKey] is the uncompressed point the app registers
 * with [RcpSurface.setNoticeKey]; the private half opens what nodes seal to it.
 */
class NoticeKey private constructor(private val pair: KeyPair) {
    val publicKey: ByteArray = uncompressedPoint(pair.public as ECPublicKey)

    /** The key as bytes to keep: the public point, then the private half as PKCS #8. Secret. */
    fun encoded(): ByteArray = publicKey + pair.private.encoded

    /**
     * Opens [notice], sealed to this key for device [deviceId]: ECDH with the envelope's ephemeral key,
     * HKDF-SHA256, then AES-256-GCM over the notice and device ids. Throws [UnreadableNotice] otherwise.
     */
    fun open(notice: HeldNotice, deviceId: Uuid): SetupLink {
        val envelope =
            runCatching { Base64.getUrlDecoder().decode(notice.sealed) }.getOrNull()
                ?: throw UnreadableNotice("the notice is not base64url")
        if (envelope.size < 1 + POINT_BYTES + NONCE_BYTES + TAG_BYTES || envelope[0] != VERSION) {
            throw UnreadableNotice("the notice is not a version $VERSION envelope")
        }
        val ephemeral = envelope.copyOfRange(1, 1 + POINT_BYTES)
        val nonce = envelope.copyOfRange(1 + POINT_BYTES, 1 + POINT_BYTES + NONCE_BYTES)
        val secret =
            try {
                KeyAgreement.getInstance("ECDH").run {
                    init(pair.private)
                    doPhase(p256PublicKey(ephemeral), true)
                    generateSecret()
                }
            } catch (failure: Exception) {
                throw UnreadableNotice("the notice's ephemeral key is not a P-256 point", failure)
            }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, SecretKeySpec(hkdfSha256(secret, ephemeral + publicKey), "AES"), GCMParameterSpec(TAG_BYTES * 8, nonce))
        cipher.updateAAD(DOMAIN + notice.noticeId.toByteArray() + deviceId.toByteArray())
        val plaintext =
            try {
                cipher.doFinal(envelope, 1 + POINT_BYTES + NONCE_BYTES, envelope.size - 1 - POINT_BYTES - NONCE_BYTES)
            } catch (forged: AEADBadTagException) {
                throw UnreadableNotice("the notice was not sealed to this device as itself", forged)
            }
        val link =
            runCatching { wireJson.decodeFromString(SetupLink.serializer(), plaintext.decodeToString()) }.getOrNull()
                ?: throw UnreadableNotice("the notice does not hold a setup link")
        if (runCatching { URI(link.url).scheme }.getOrNull() != "https") throw UnreadableNotice("the setup link is not https")
        return link
    }

    companion object {
        fun generate(): NoticeKey = NoticeKey(KeyPairGenerator.getInstance("EC").run { initialize(ECGenParameterSpec("secp256r1")); generateKeyPair() })

        /** The key [encoded] wrote. Throws when [bytes] are not one. */
        fun decode(bytes: ByteArray): NoticeKey {
            require(bytes.size > POINT_BYTES) { "a kept notice key is its point and its private half" }
            val private = KeyFactory.getInstance("EC").generatePrivate(PKCS8EncodedKeySpec(bytes.copyOfRange(POINT_BYTES, bytes.size)))
            return NoticeKey(KeyPair(p256PublicKey(bytes.copyOfRange(0, POINT_BYTES)), private))
        }
    }
}

/** HKDF-SHA256 with [DOMAIN] as salt, expanded to one 32-byte block: the AES-256 key. */
private fun hkdfSha256(secret: ByteArray, info: ByteArray): ByteArray {
    fun hmac(key: ByteArray, data: ByteArray) = Mac.getInstance("HmacSHA256").run { init(SecretKeySpec(key, "HmacSHA256")); doFinal(data) }
    return hmac(hmac(DOMAIN, secret), info + 1.toByte())
}

private val P256: ECParameterSpec =
    AlgorithmParameters.getInstance("EC").run {
        init(ECGenParameterSpec("secp256r1"))
        getParameterSpec(ECParameterSpec::class.java)
    }

/** The P-256 public key whose uncompressed point is [point]. */
internal fun p256PublicKey(point: ByteArray): ECPublicKey {
    require(point.size == POINT_BYTES && point[0] == 0x04.toByte()) { "a P-256 public key is a 65-byte uncompressed point" }
    val x = BigInteger(1, point.copyOfRange(1, 33))
    val y = BigInteger(1, point.copyOfRange(33, 65))
    return KeyFactory.getInstance("EC").generatePublic(ECPublicKeySpec(ECPoint(x, y), P256)) as ECPublicKey
}

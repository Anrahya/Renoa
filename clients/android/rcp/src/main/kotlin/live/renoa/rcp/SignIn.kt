package live.renoa.rcp

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import java.io.IOException
import java.math.BigInteger
import java.security.interfaces.ECPublicKey
import kotlin.uuid.Uuid

/**
 * The key a device signs in with and proves on every connection. [publicKey] is the 65-byte
 * uncompressed P-256 point; [sign] returns an ECDSA SHA-256 signature, ASN.1 DER, made with the
 * private half, which never leaves the device.
 */
class DeviceKey(val publicKey: ByteArray, val sign: suspend (ByteArray) -> ByteArray) {
    init {
        require(publicKey.size == 65 && publicKey[0] == 0x04.toByte()) { "a device key is a 65-byte uncompressed P-256 point" }
    }
}

/** The uncompressed point `0x04 || X || Y` of a P-256 public key, as the coordinator registers it. */
fun uncompressedPoint(key: ECPublicKey): ByteArray {
    fun coordinate(value: BigInteger): ByteArray {
        val bytes = value.toByteArray().dropWhile { it == 0.toByte() }.toByteArray()
        require(bytes.size <= 32) { "not a P-256 coordinate" }
        return ByteArray(32 - bytes.size) + bytes
    }
    return byteArrayOf(0x04) + coordinate(key.w.affineX) + coordinate(key.w.affineY)
}

/** The text a key signs to bind itself to one passkey ceremony, as `docs/identity-v0.md` defines it. */
internal fun deviceRegistrationText(ceremonyId: Uuid): ByteArray =
    "renoa device registration v1\n$ceremonyId".toByteArray(Charsets.UTF_8)

/** The coordinator refused a sign-in. [code] is its error code, such as `conflict` or `not_found`. */
class SignInRefused(val status: Int, val code: String, message: String) : Exception(message)

/**
 * The answer to a ceremony's last step was lost. The coordinator claims a ceremony when that step
 * arrives, so it may have signed the device in, and the same setup code cannot be tried again.
 */
class SignInUncertain(cause: Throwable) : Exception("the sign-in's last answer was lost", cause)

/** A registration whose options were fetched, which spent its setup code, and whose prompt has not finished. */
private class PendingRegistration(val bootstrapToken: String, val publicKey: ByteArray, val ceremonyId: Uuid, val options: String)

/**
 * Signs an app in with a passkey and a device key over the coordinator's identity HTTP API.
 *
 * The passkey prompt belongs to the platform, so the caller supplies it: it receives the WebAuthn
 * options as JSON (`PublicKeyCredentialCreationOptionsJSON` or `…RequestOptionsJSON`) and returns
 * the credential the platform made, as the WebAuthn JSON a browser's `toJSON()` produces. A
 * successful sign-in returns the device's credentials for [WebSocketRcpSurface].
 *
 * Fetching registration options spends the setup code, so a registration whose prompt was cancelled
 * or failed is kept and the next [register] with the same code and key prompts again for it,
 * within the coordinator's five-minute ceremony. One instance serves one sign-in screen; its calls
 * must not overlap.
 */
class PasskeySignIn(
    origin: String,
    private val surface: String,
    private val label: String,
    http: OkHttpClient = OkHttpClient(),
) {
    // The ceremonies carry a bootstrap token and return a bearer credential, so a redirect must not
    // carry them to a host the caller never named.
    private val http = http.newBuilder().followRedirects(false).followSslRedirects(false).build()
    private val base = origin.trimEnd('/')
    private var pending: PendingRegistration? = null

    init {
        requireSecureEndpoint(base)
        require(label.isNotBlank() && label.codePointCount(0, label.length) <= 64 && label.none(Char::isISOControl)) {
            "a device label is 1 to 64 printable characters"
        }
    }

    /** A new passkey, authorized by a one-use bootstrap token from the Renoa operator. */
    suspend fun register(bootstrapToken: String, key: DeviceKey, create: suspend (String) -> String): DeviceCredentials {
        val ceremony =
            pending?.takeIf { it.bootstrapToken == bootstrapToken && it.publicKey.contentEquals(key.publicKey) }
                ?: options("registration", key) { put("bootstrapToken", bootstrapToken) }
                    .let { (id, options) -> PendingRegistration(bootstrapToken, key.publicKey, id, options) }
        pending = ceremony
        val body = verifyBody(ceremony.ceremonyId, key, create(ceremony.options))
        // The coordinator claims the ceremony as the verify request arrives, so it is never sent twice.
        pending = null
        return verify("registration", body)
    }

    /** A passkey the person already has, for the principal it belongs to. */
    suspend fun authenticate(principalId: Uuid, key: DeviceKey, get: suspend (String) -> String): DeviceCredentials {
        val (ceremonyId, options) = options("authentication", key) { put("principalId", principalId.toString()) }
        return verify("authentication", verifyBody(ceremonyId, key, get(options)))
    }

    private suspend fun options(
        kind: String,
        key: DeviceKey,
        authority: kotlinx.serialization.json.JsonObjectBuilder.() -> Unit,
    ): Pair<Uuid, String> {
        val options =
            post(
                "/v1/identity/passkeys/$kind/options",
                buildJsonObject {
                    authority()
                    put("surface", surface)
                    putJsonObject("device") {
                        put("publicKey", base64Url(key.publicKey))
                        put("label", label)
                    }
                },
            )
        val ceremonyId =
            options.string("ceremonyId")?.let(Uuid::parse) ?: throw IOException("the coordinator's passkey options have no ceremonyId")
        val publicKey =
            (options["options"] as? JsonObject)?.get("publicKey") as? JsonObject
                ?: throw IOException("the coordinator's passkey options have no publicKey")
        return ceremonyId to publicKey.toString()
    }

    private suspend fun verifyBody(ceremonyId: Uuid, key: DeviceKey, credential: String): JsonObject {
        val parsed = wireJson.parseToJsonElement(credential)
        val proof = base64Url(key.sign(deviceRegistrationText(ceremonyId)))
        return buildJsonObject {
            put("ceremonyId", ceremonyId.toString())
            put("credential", parsed)
            put("deviceProof", proof)
        }
    }

    private suspend fun verify(kind: String, body: JsonObject): DeviceCredentials =
        try {
            val verified = post("/v1/identity/passkeys/$kind/verify", body)
            val credentials = verified["credentials"] ?: throw IOException("the coordinator's sign-in answer has no credentials")
            wireJson.decodeFromJsonElement(DeviceCredentials.serializer(), credentials)
        } catch (lost: IOException) {
            throw SignInUncertain(lost)
        } catch (malformed: IllegalArgumentException) {
            throw SignInUncertain(malformed)
        }

    private suspend fun post(path: String, body: JsonObject): JsonObject =
        withContext(Dispatchers.IO) {
            val request =
                Request.Builder()
                    .url(base + path)
                    .post(body.toString().toRequestBody("application/json".toMediaType()))
                    .build()
            http.newCall(request).execute().use { response ->
                val text = response.body.string()
                val json = runCatching { wireJson.parseToJsonElement(text).jsonObject }.getOrNull()
                if (!response.isSuccessful) {
                    throw SignInRefused(
                        response.code,
                        json?.string("code") ?: "http_${response.code}",
                        json?.string("message") ?: "the coordinator answered ${response.code}",
                    )
                }
                json ?: throw IOException("the coordinator's answer to $path is not a JSON object")
            }
        }

    private fun JsonObject.string(name: String): String? = runCatching { this[name]?.jsonPrimitive?.contentOrNull }.getOrNull()
}

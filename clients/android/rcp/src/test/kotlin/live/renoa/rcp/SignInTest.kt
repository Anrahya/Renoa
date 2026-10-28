package live.renoa.rcp

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import mockwebserver3.Dispatcher
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import mockwebserver3.RecordedRequest
import java.math.BigInteger
import java.security.AlgorithmParameters
import java.security.KeyFactory
import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.PublicKey
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.security.spec.ECParameterSpec
import java.security.spec.ECPoint
import java.security.spec.ECPublicKeySpec
import java.util.Base64
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlin.uuid.Uuid

/** Passkey sign-in with a device key, against an identity server written from docs/identity-v0.md. */
class SignInTest {
    private val ceremonyId = Uuid.random()
    private val issued = DeviceCredentials(Uuid.random(), "d".repeat(64))
    private val passkeyOptions = """{"challenge":"Y2hhbGxlbmdl","rp":{"id":"renoa.live","name":"Renoa"}}"""
    private val requests = CopyOnWriteArrayList<Pair<RecordedRequest, JsonObject>>()

    /** When set, the options request is refused with this status and error code. */
    @Volatile private var refuse: Pair<Int, String>? = null

    /** When set, every request is answered with this instead. */
    @Volatile private var answerWith: MockResponse? = null

    /** The fields each request must carry, exactly, as the coordinator's request structs read them. */
    private val expectedFields =
        mapOf(
            "/v1/identity/passkeys/registration/options" to setOf("bootstrapToken", "surface", "device"),
            "/v1/identity/passkeys/authentication/options" to setOf("principalId", "surface", "device"),
            "/v1/identity/passkeys/registration/verify" to setOf("ceremonyId", "credential", "deviceProof"),
            "/v1/identity/passkeys/authentication/verify" to setOf("ceremonyId", "credential", "deviceProof"),
        )

    private val server =
        MockWebServer().apply {
            dispatcher =
                object : Dispatcher() {
                    override fun dispatch(request: RecordedRequest): MockResponse {
                        val body = Json.parseToJsonElement(request.body!!.utf8()).jsonObject
                        requests += request to body
                        answerWith?.let { return it }
                        val path = request.url.encodedPath
                        if (body.keys != expectedFields[path] ||
                            (path.endsWith("/options") && body.getValue("device").jsonObject.keys != setOf("publicKey", "label"))
                        ) {
                            return MockResponse.Builder().code(400).body("""{"code":"invalid_request","message":"unexpected fields"}""").build()
                        }
                        refuse?.let { (status, code) ->
                            return MockResponse.Builder().code(status).body("""{"code":"$code","message":"refused: $code"}""").build()
                        }
                        val answer =
                            if (request.url.encodedPath.endsWith("/options")) {
                                """{"ceremonyId":"$ceremonyId","options":{"publicKey":$passkeyOptions}}"""
                            } else {
                                """{"credentials":{"deviceId":"${issued.deviceId}","credential":"${issued.credential}"}}"""
                            }
                        return MockResponse.Builder().body(answer).build()
                    }
                }
            start()
        }

    @AfterTest
    fun tearDown() = server.close()

    private fun p256(): KeyPair =
        KeyPairGenerator.getInstance("EC").run {
            initialize(ECGenParameterSpec("secp256r1"))
            generateKeyPair()
        }

    private fun deviceKey(pair: KeyPair) =
        DeviceKey(uncompressedPoint(pair.public as ECPublicKey)) { bytes ->
            Signature.getInstance("SHA256withECDSA").run {
                initSign(pair.private)
                update(bytes)
                sign()
            }
        }

    private fun signIn() = PasskeySignIn("http://127.0.0.1:${server.port}", surface = "android", label = "Tablet")

    private fun publicKeyOf(point: ByteArray): PublicKey {
        val curve =
            AlgorithmParameters.getInstance("EC").run {
                init(ECGenParameterSpec("secp256r1"))
                getParameterSpec(ECParameterSpec::class.java)
            }
        val x = BigInteger(1, point.copyOfRange(1, 33))
        val y = BigInteger(1, point.copyOfRange(33, 65))
        return KeyFactory.getInstance("EC").generatePublic(ECPublicKeySpec(ECPoint(x, y), curve))
    }

    private fun verifies(key: PublicKey, text: ByteArray, signature: String) =
        Signature.getInstance("SHA256withECDSA").run {
            initVerify(key)
            update(text)
            verify(Base64.getUrlDecoder().decode(signature))
        }

    @Test
    fun aRegistrationBindsTheKeyToTheCeremonyAndReturnsTheDevicesCredentials() = eventually {
        val pair = p256()
        var shown: String? = null
        val credentials =
            signIn().register("ab".repeat(32), deviceKey(pair)) { options ->
                shown = options
                """{"id":"cred","rawId":"cred","type":"public-key","response":{"clientDataJSON":"e30","attestationObject":"o2M"}}"""
            }

        assertEquals(issued, credentials)
        assertEquals(Json.parseToJsonElement(passkeyOptions), Json.parseToJsonElement(shown!!), "the prompt gets the inner publicKey options")
        val (optionsRequest, options) = requests[0]
        assertEquals("/v1/identity/passkeys/registration/options", optionsRequest.url.encodedPath)
        assertNull(optionsRequest.headers["Origin"], "a native client sends no Origin")
        assertEquals("ab".repeat(32), options.getValue("bootstrapToken").jsonPrimitive.content)
        assertEquals("android", options.getValue("surface").jsonPrimitive.content)
        val device = options.getValue("device").jsonObject
        assertEquals("Tablet", device.getValue("label").jsonPrimitive.content)
        val point = Base64.getUrlDecoder().decode(device.getValue("publicKey").jsonPrimitive.content)
        assertTrue('=' !in device.getValue("publicKey").jsonPrimitive.content)

        val (verifyRequest, verify) = requests[1]
        assertEquals("/v1/identity/passkeys/registration/verify", verifyRequest.url.encodedPath)
        assertEquals(ceremonyId.toString(), verify.getValue("ceremonyId").jsonPrimitive.content)
        assertEquals("cred", verify.getValue("credential").jsonObject.getValue("id").jsonPrimitive.content, "the credential is sent as JSON, not a string")
        val proof = verify.getValue("deviceProof").jsonPrimitive.content
        assertTrue(verifies(publicKeyOf(point), deviceRegistrationText(ceremonyId), proof), "the proof is over the registration message with the registered key")
    }

    @Test
    fun anExistingPasskeySignsInForItsPrincipal() = eventually {
        val principal = Uuid.random()
        val credentials = signIn().authenticate(principal, deviceKey(p256())) { """{"id":"cred","type":"public-key","response":{}}""" }
        assertEquals(issued, credentials)
        val (request, options) = requests[0]
        assertEquals("/v1/identity/passkeys/authentication/options", request.url.encodedPath)
        assertEquals(principal.toString(), options.getValue("principalId").jsonPrimitive.content)
        assertEquals("/v1/identity/passkeys/authentication/verify", requests[1].first.url.encodedPath)
    }

    @Test
    fun aRefusedCeremonyNamesTheCoordinatorsCodeAndNeverPrompts() = eventually {
        refuse = 409 to "conflict"
        var prompted = false
        val refused =
            assertFailsWith<SignInRefused> {
                signIn().register("ab".repeat(32), deviceKey(p256())) { prompted = true; "{}" }
            }
        assertEquals(409, refused.status)
        assertEquals("conflict", refused.code)
        assertEquals(false, prompted)
    }

    @Test
    fun aCancelledPromptIsAskedAgainForTheSameCeremonyWithoutSpendingTheCode() = eventually {
        val key = deviceKey(p256())
        val client = signIn()
        assertFailsWith<IllegalStateException> { client.register("ab".repeat(32), key) { error("closed the prompt") } }
        val credentials = client.register("ab".repeat(32), key) { """{"id":"cred","type":"public-key","response":{}}""" }

        assertEquals(issued, credentials)
        assertEquals(1, requests.count { it.first.url.encodedPath.endsWith("/registration/options") }, "the code was spent once")
        assertEquals(ceremonyId.toString(), requests.last().second.getValue("ceremonyId").jsonPrimitive.content)

        // The ceremony is bound to the key it named, so a new key is a new ceremony.
        assertFailsWith<IllegalStateException> { client.register("cd".repeat(32), key) { error("closed the prompt") } }
        client.register("cd".repeat(32), deviceKey(p256())) { "{}" }
        assertEquals(3, requests.count { it.first.url.encodedPath.endsWith("/registration/options") })
    }

    @Test
    fun aLostAnswerToTheLastStepIsUncertainAndTheCeremonyIsNotReused() = eventually {
        val key = deviceKey(p256())
        val client = signIn()
        val prompt: suspend (String) -> String = { options ->
            answerWith = MockResponse.Builder().body("not json").build()
            options.length.toString()
        }
        assertFailsWith<SignInUncertain> { client.register("ab".repeat(32), key, prompt) }
        answerWith = null
        client.register("ab".repeat(32), key) { "{}" }
        assertEquals(2, requests.count { it.first.url.encodedPath.endsWith("/registration/options") }, "a claimed ceremony is never replayed")
    }

    @Test
    fun aRedirectIsNeverFollowedWithTheCode() = eventually {
        answerWith = MockResponse.Builder().code(307).addHeader("Location", "http://127.0.0.1:${server.port}/elsewhere").build()
        val refused = assertFailsWith<SignInRefused> { signIn().register("ab".repeat(32), deviceKey(p256())) { "{}" } }
        assertEquals(307, refused.status)
        assertEquals(1, requests.size)
    }

    @Test
    fun theKeyIsRegisteredAsTheUncompressedPointJavaEncodes() {
        // About one key in 128 has a coordinate whose top byte is zero, which must still take 32 bytes.
        val short = BigInteger.ONE.shiftLeft(248)
        val key =
            generateSequence { p256().public as ECPublicKey }
                .take(5_000)
                .first { it.w.affineX < short || it.w.affineY < short }
        // A P-256 SubjectPublicKeyInfo ends with the 65-byte uncompressed point.
        assertContentEquals(key.encoded.takeLast(65).toByteArray(), uncompressedPoint(key))
        val ordinary = p256().public as ECPublicKey
        assertContentEquals(ordinary.encoded.takeLast(65).toByteArray(), uncompressedPoint(ordinary))
    }

    @Test
    fun credentialsAreNeverSentInPlainTextToAnotherHost() {
        assertFailsWith<IllegalArgumentException> { PasskeySignIn("http://renoa.live", surface = "android", label = "Tablet") }
        assertFailsWith<IllegalArgumentException> { PasskeySignIn("https://renoa.live", surface = "android", label = "") }
    }
}

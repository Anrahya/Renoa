package live.renoa.rcp

import java.io.File
import java.math.BigInteger
import java.security.AlgorithmParameters
import java.security.KeyFactory
import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.Signature
import java.security.spec.ECGenParameterSpec
import java.security.spec.ECParameterSpec
import java.security.spec.ECPoint
import java.security.spec.ECPublicKeySpec
import java.util.Base64
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import kotlin.test.assertTrue
import kotlin.uuid.Uuid

/** A device that registered a key at sign-in proves it on every connection. */
class DeviceKeyTest : CoordinatorTest() {
    private fun p256(): KeyPair =
        KeyPairGenerator.getInstance("EC").run {
            initialize(ECGenParameterSpec("secp256r1"))
            generateKeyPair()
        }

    /** Signs as Android Keystore's SHA256withECDSA does: ASN.1 DER. */
    private fun signer(key: KeyPair): suspend (ByteArray) -> ByteArray = { bytes ->
        Signature.getInstance("SHA256withECDSA").run {
            initSign(key.private)
            update(bytes)
            sign()
        }
    }

    @Test
    fun aDeviceWithItsKeyAnswersTheChallengeAndConnects() = eventually {
        val key = p256()
        coordinator.deviceKey = key.public
        val surface = client(Authentication.Device(coordinator.credentials, signer(key)))
        surface.connect()
        assertEquals(emptyList(), surface.listTasks())
        assertEquals(1, coordinator.framesOfType("device_proof").size)
    }

    @Test
    fun anotherKeysSignatureIsRefused() = eventually {
        coordinator.deviceKey = p256().public
        val surface = client(Authentication.Device(coordinator.credentials, signer(p256())))
        assertEquals(RcpErrorCode.AuthenticationFailed, assertFailsWith<RcpException> { surface.connect() }.code)
    }

    @Test
    fun aDeviceGivenNoKeyIsToldToSignInAgainAndSendsNoProof() = eventually {
        coordinator.deviceKey = p256().public
        val surface = client(Authentication.Device(coordinator.credentials))
        assertFailsWith<DeviceKeyRequired> { surface.connect() }
        assertEquals(0, coordinator.framesOfType("device_proof").size)
    }

    @Test
    fun aChallengeWithAMalformedNonceIsNotSigned() = eventually {
        val key = p256()
        coordinator.deviceKey = key.public
        coordinator.nonceOverride = "AB".repeat(32)
        var signed = false
        val surface = client(Authentication.Device(coordinator.credentials) { signed = true; signer(key)(it) })
        assertFailsWith<ProtocolViolation> { surface.connect() }
        assertEquals(false, signed)
    }

    @Test
    fun aChallengeAfterAuthenticationEndsTheSession() = eventually {
        val surface = client()
        surface.connect()
        coordinator.injectRaw("""{"type":"device_challenge","nonce":"${"ab".repeat(32)}"}""")
        assertIs<ProtocolViolation>(surface.awaitDisconnect())
        assertEquals(0, coordinator.framesOfType("device_proof").size)
    }

    /** The coordinator's checked-in vector, read from docs/identity-v0.md, verifies against the client's proof text and encoding. */
    @Test
    fun theDocumentedVectorsVerifyAgainstTheClientsTexts() {
        val doc = File(System.getProperty("renoa.docs"), "identity-v0.md").readText()
        val key = Regex("The public key is\\s+`([^`]+)`").find(doc)!!.groupValues[1]
        val (device, nonce, proof) =
            Regex("The proof for device `([^`]+)`\\s+and nonce\\s+`([^`]+)` is\\s+`([^`]+)`").find(doc)!!.destructured
        val point = Base64.getUrlDecoder().decode(key)
        val curve =
            AlgorithmParameters.getInstance("EC").run {
                init(ECGenParameterSpec("secp256r1"))
                getParameterSpec(ECParameterSpec::class.java)
            }
        val x = BigInteger(1, point.copyOfRange(1, 33))
        val y = BigInteger(1, point.copyOfRange(33, 65))
        val publicKey = KeyFactory.getInstance("EC").generatePublic(ECPublicKeySpec(ECPoint(x, y), curve))
        val der = Base64.getUrlDecoder().decode(proof)
        val verifies =
            Signature.getInstance("SHA256withECDSA").run {
                initVerify(publicKey)
                update(deviceProofText(Uuid.parse(device), nonce))
                verify(der)
            }
        assertTrue(verifies, "the documented proof does not verify over the client's proof text")
        assertEquals(proof, base64Url(der), "the client must encode a signature exactly as the coordinator reads it")
        val (ceremony, registration) =
            Regex("The registration proof for ceremony `([^`]+)` is\\s+`([^`]+)`").find(doc)!!.destructured
        val registers =
            Signature.getInstance("SHA256withECDSA").run {
                initVerify(publicKey)
                update(deviceRegistrationText(Uuid.parse(ceremony)))
                verify(Base64.getUrlDecoder().decode(registration))
            }
        assertTrue(registers, "the documented registration proof does not verify over the client's registration text")
    }
}

package live.renoa.rcp

import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.Signature
import java.security.spec.ECGenParameterSpec
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs

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
}

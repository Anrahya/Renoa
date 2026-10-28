package live.renoa.app.signin

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import live.renoa.rcp.DeviceKey
import live.renoa.rcp.uncompressedPoint
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec

/**
 * This installation's key in the Android Keystore. The private half never leaves the Keystore. It
 * signs without asking the person, because a reconnect from the background has no one to unlock
 * the phone.
 */
internal object DeviceKeys {
    private const val ALIAS = "renoa-device-key"

    private fun keystore(): KeyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    suspend fun getOrCreate(): DeviceKey =
        withContext(Dispatchers.IO) {
            val store = keystore()
            if (!store.containsAlias(ALIAS)) {
                KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore").run {
                    initialize(
                        KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_SIGN)
                            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
                            .setDigests(KeyProperties.DIGEST_SHA256)
                            .build(),
                    )
                    generateKeyPair()
                }
            }
            val publicKey = store.getCertificate(ALIAS).publicKey as ECPublicKey
            DeviceKey(uncompressedPoint(publicKey)) { bytes ->
                withContext(Dispatchers.IO) {
                    Signature.getInstance("SHA256withECDSA").run {
                        initSign(keystore().getKey(ALIAS, null) as PrivateKey)
                        update(bytes)
                        sign()
                    }
                }
            }
        }

    /** Drops the key, so the next sign-in registers a new one. */
    suspend fun forget() = withContext(Dispatchers.IO) { keystore().deleteEntry(ALIAS) }
}

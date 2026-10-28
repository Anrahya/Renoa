package live.renoa.app.signin

import android.annotation.SuppressLint
import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import live.renoa.rcp.DeviceCredentials
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Where this app signed in, and the device credential the coordinator issued for it. */
@Serializable
internal data class SavedSignIn(val origin: String, val credentials: DeviceCredentials)

/**
 * Keeps [SavedSignIn] across restarts. The credential is a bearer secret, so it is stored encrypted
 * with an AES key that never leaves the Android Keystore; backups are off for the whole app.
 */
internal class SignInStore(context: Context) {
    private val preferences = context.getSharedPreferences("sign-in", Context.MODE_PRIVATE)

    fun load(): SavedSignIn? {
        val stored = preferences.getString(ENTRY, null) ?: return null
        // A key the Keystore lost, as after a restore to a new phone, means signing in again.
        return runCatching {
            val bytes = Base64.decode(stored, Base64.NO_WRAP)
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, bytes, 0, IV_BYTES))
            Json.decodeFromString(SavedSignIn.serializer(), String(cipher.doFinal(bytes, IV_BYTES, bytes.size - IV_BYTES), Charsets.UTF_8))
        }.getOrNull()
    }

    // The KTX edit drops commit's result, and a sign-in that did not reach the disk must not count.
    @SuppressLint("UseKtx")
    fun save(signIn: SavedSignIn) {
        val cipher = Cipher.getInstance(TRANSFORMATION).apply { init(Cipher.ENCRYPT_MODE, key()) }
        val sealed = cipher.iv + cipher.doFinal(Json.encodeToString(SavedSignIn.serializer(), signIn).toByteArray(Charsets.UTF_8))
        check(cipher.iv.size == IV_BYTES)
        // commit, not apply: the app moves on only once the credential is on disk.
        check(preferences.edit().putString(ENTRY, Base64.encodeToString(sealed, Base64.NO_WRAP)).commit()) { "could not store the sign-in" }
    }

    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
            init(
                KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .build(),
            )
            generateKey()
        }
    }

    private companion object {
        const val ENTRY = "signed-in"
        const val KEY_ALIAS = "renoa-sign-in-store"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val IV_BYTES = 12
    }
}

package live.renoa.app.signin

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import live.renoa.rcp.NoticeKey
import java.io.File
import java.security.GeneralSecurityException
import java.security.KeyStore
import java.security.KeyStoreException
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * This installation's key for setup notices, kept in [file] sealed by an AES key in the Android
 * Keystore. The Keystore holds no P-256 agreement key below Android 12, so the notice key itself is
 * kept wrapped instead, the same on every version; its private half is in memory only while a
 * notice is opened.
 */
internal class NoticeKeys(private val file: File) {
    private val keeping = Mutex()

    suspend fun getOrCreate(): NoticeKey =
        keeping.withLock {
            withContext(Dispatchers.IO) {
                // A kept key that no longer opens, as when the Keystore lost its wrapping key, is replaced:
                // Renoa then holds the new one, and notices sealed to the old one are acknowledged away.
                if (file.exists()) {
                    try {
                        return@withContext NoticeKey.decode(unwrap(file.readBytes()))
                    } catch (lost: GeneralSecurityException) {
                        // A Keystore that is only busy throws KeyStoreException or ProviderException; those go to the caller and the key stays.
                        if (lost is KeyStoreException) throw lost
                    } catch (unreadable: IllegalArgumentException) {
                        // The kept bytes are not a key.
                    }
                }
                NoticeKey.generate().also { key ->
                    // Written whole, then moved into place, so a crash never leaves half a key.
                    val next = File(file.parentFile, "${file.name}.new")
                    next.writeBytes(wrap(key.encoded()))
                    check(next.renameTo(file)) { "could not keep the notice key" }
                }
            }
        }

    /** Drops the key, so the next sign-in registers a new one and no notice sealed to this one opens. */
    suspend fun forget() =
        keeping.withLock {
            withContext(Dispatchers.IO) {
                file.delete()
                keystore().deleteEntry(ALIAS)
            }
        }

    private fun wrap(bytes: ByteArray): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORMATION).apply { init(Cipher.ENCRYPT_MODE, wrappingKey()) }
        return cipher.iv + cipher.doFinal(bytes)
    }

    private fun unwrap(bytes: ByteArray): ByteArray =
        Cipher.getInstance(TRANSFORMATION)
            .apply { init(Cipher.DECRYPT_MODE, wrappingKey(), GCMParameterSpec(128, bytes, 0, IV_BYTES)) }
            .doFinal(bytes, IV_BYTES, bytes.size - IV_BYTES)

    private fun wrappingKey(): SecretKey =
        (keystore().getKey(ALIAS, null) as SecretKey?)
            ?: KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
                init(
                    KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                        .build(),
                )
                generateKey()
            }

    private fun keystore(): KeyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    private companion object {
        const val ALIAS = "renoa-notice-wrap"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val IV_BYTES = 12
    }
}

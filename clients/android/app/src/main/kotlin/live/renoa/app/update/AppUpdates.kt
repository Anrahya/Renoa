package live.renoa.app.update

import android.content.Context
import android.content.Intent
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.core.content.FileProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import live.renoa.app.BuildConfig
import live.renoa.app.SHARED_FILES
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.OkHttpClient
import okhttp3.Request
import java.io.File
import java.io.IOException
import java.security.MessageDigest

/** The newest app Renoa serves, as renoa-deploy describes the one its release shipped. */
@Serializable
data class AppRelease(
    @SerialName("version_code") val versionCode: Int,
    @SerialName("version_name") val versionName: String,
    val apk: String,
    val sha256: String,
    val size: Long,
)

/** The name renoa-deploy gives a release's app, so a description cannot point the download anywhere else. */
private val APK_NAME = Regex("""renoa-\d{1,3}\.\d{1,3}\.\d{1,3}\.apk""")

private val described = Json { ignoreUnknownKeys = true }

/**
 * Finds a newer app on Renoa and downloads it into [dir], checked against its size and digest, for
 * the phone's installer. The description sits at [releaseUrl] and the app beside it; [installed] is
 * this app's version code.
 */
class AppUpdates(
    private val dir: File,
    private val releaseUrl: String = BuildConfig.RENOA_APP_RELEASE,
    private val installed: Int = BuildConfig.VERSION_CODE,
    private val http: OkHttpClient = OkHttpClient(),
) {
    /** The app Renoa serves when it is newer than this one; null when it is not, or Renoa serves none. */
    suspend fun newer(): AppRelease? =
        withContext(Dispatchers.IO) {
            http.newCall(Request.Builder().url(releaseUrl).build()).execute().use { response ->
                if (response.code == 404) return@use null
                if (!response.isSuccessful) throw IOException("checking for an update failed with ${response.code}")
                described.decodeFromString<AppRelease>(response.body.string())
            }?.takeIf { it.versionCode > installed }
        }

    /** One download at a time: a second tap, or a screen made again by a rotation, waits for the first. */
    private val downloading = Mutex()

    /**
     * Downloads [release]'s app and returns it once its size and digest match. A mismatch throws and
     * keeps nothing, and a download that starts drops the one kept before it.
     */
    suspend fun download(release: AppRelease): File =
        downloading.withLock {
            withContext(Dispatchers.IO) { fetch(release) }
        }

    private suspend fun fetch(release: AppRelease): File {
        if (!APK_NAME.matches(release.apk)) throw IOException("the update is named ${release.apk}")
        dir.mkdirs()
        val kept = File(dir, "renoa.apk")
        kept.delete()
        val partial = File(dir, "renoa.apk.part")
        val digest = MessageDigest.getInstance("SHA-256")
        try {
            val url = checkNotNull(releaseUrl.toHttpUrl().resolve(release.apk))
            http.newCall(Request.Builder().url(url).build()).execute().use { response ->
                if (!response.isSuccessful) throw IOException("the update download failed with ${response.code}")
                var written = 0L
                response.body.byteStream().use { input ->
                    partial.outputStream().use { output ->
                        val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
                        while (true) {
                            currentCoroutineContext().ensureActive()
                            val read = input.read(buffer)
                            if (read < 0) break
                            written += read
                            if (written > release.size) throw IOException("the update is larger than described")
                            digest.update(buffer, 0, read)
                            output.write(buffer, 0, read)
                        }
                    }
                }
                val hex = digest.digest().joinToString("") { "%02x".format(it) }
                if (written != release.size || hex != release.sha256) throw IOException("the update does not match its description")
            }
            return kept.also { if (!partial.renameTo(it)) throw IOException("the update could not be kept") }
        } finally {
            partial.delete()
        }
    }
}

/** Hands a downloaded update to the phone's installer, which asks the owner, and the first time to allow it. */
fun install(context: Context, apk: File) {
    val uri = FileProvider.getUriForFile(context, context.packageName + SHARED_FILES, apk)
    context.startActivity(
        Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, "application/vnd.android.package-archive")
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK),
    )
}

/** Where Settings looks for a newer app: none in tests and previews, so they never reach renoa.live. */
val LocalAppUpdates = staticCompositionLocalOf<AppUpdates?> { null }

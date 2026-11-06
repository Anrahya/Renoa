package live.renoa.app.update

import kotlinx.coroutines.test.runTest
import mockwebserver3.Dispatcher
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import mockwebserver3.RecordedRequest
import org.junit.After
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.IOException
import java.security.MessageDigest
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** The app finds a newer release on renoa.live and keeps its download only when it matches. */
class AppUpdatesTest {
    @get:Rule val folder = TemporaryFolder()

    private val apk = "the app".toByteArray()
    private val served = mutableMapOf<String, MockResponse>()
    private val asked = mutableListOf<String>()
    private val server =
        MockWebServer().apply {
            dispatcher =
                object : Dispatcher() {
                    override fun dispatch(request: RecordedRequest): MockResponse {
                        val path = request.url.encodedPath
                        asked += path
                        return served[path] ?: MockResponse.Builder().code(404).build()
                    }
                }
            start()
        }

    @After fun stop() = server.close()

    private fun updates(installed: Int) = AppUpdates(folder.root, server.url("/app/android.json").toString(), installed)

    private fun describe(code: Int, digest: String = sha256(apk), size: Int = apk.size, name: String = "renoa-0.16.0.apk") {
        val body = """{"version_code": $code, "version_name": "0.16.0", "apk": "$name", "sha256": "$digest", "size": $size, "later": true}"""
        served["/app/android.json"] = MockResponse.Builder().body(body).build()
        served["/app/renoa-0.16.0.apk"] = MockResponse.Builder().body(okio.Buffer().write(apk)).build()
    }

    @Test
    fun onlyANewerReleaseIsOfferedAndNoneServedIsNoneToOffer() = runTest {
        assertNull(updates(installed = 1).newer(), "Renoa serves no app yet")
        describe(code = 16000)
        assertNull(updates(installed = 16000).newer(), "the installed app is this release")
        assertEquals("0.16.0", updates(installed = 15002).newer()?.versionName)
        served["/app/android.json"] = MockResponse.Builder().code(503).build()
        assertFailsWith<IOException> { updates(installed = 1).newer() }
    }

    @Test
    fun aDownloadIsKeptOnlyWhenItsSizeAndDigestMatch() = runTest {
        describe(code = 16000)
        val release = checkNotNull(updates(installed = 1).newer())
        val kept = updates(installed = 1).download(release)
        assertEquals("the app", kept.readText())

        kept.delete()
        for (wrong in listOf(release.copy(sha256 = "0".repeat(64)), release.copy(size = 3L), release.copy(size = 99L))) {
            assertFailsWith<IOException> { updates(installed = 1).download(wrong) }
            assertTrue(folder.root.listFiles().orEmpty().isEmpty(), "a mismatch keeps nothing: ${folder.root.list()?.toList()}")
        }
    }

    @Test
    fun aDescriptionCannotSendTheDownloadAnywhereElse() = runTest {
        describe(code = 16000, name = "../elsewhere.apk")
        val release = checkNotNull(updates(installed = 1).newer())
        asked.clear()
        assertFailsWith<IOException> { updates(installed = 1).download(release) }
        assertEquals(emptyList(), asked)
    }

    private fun sha256(bytes: ByteArray) = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
}

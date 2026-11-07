package live.renoa.app.chat

import android.webkit.MimeTypeMap
import android.webkit.WebResourceRequest
import android.webkit.WebView
import androidx.compose.ui.platform.UriHandler
import androidx.test.core.app.ApplicationProvider
import kotlinx.coroutines.test.runTest
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import mockwebserver3.SocketEffect
import okhttp3.MediaType
import okhttp3.OkHttpClient
import okhttp3.ResponseBody
import okio.Buffer
import okio.BufferedSource
import org.junit.After
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.io.File
import java.io.IOException
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

/**
 * A shared file's page goes nowhere on its own, and a file the phone has no page for is fetched
 * whole, named as Renoa names it, one at a time.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [36])
class SharedFileViewTest {
    @get:Rule val folder = TemporaryFolder()

    private val server = MockWebServer()

    @Before
    fun start() = server.start()

    @After
    fun stop() = server.close()

    private fun dir() = File(folder.root, "shared")

    @Test
    fun aFileIsNamedAsRenoaNamesItAndNeverOutsideItsFolder() {
        assertEquals("Q3 plan.pdf", sharedName("inline; filename*=UTF-8''Q3%20plan.pdf"))
        assertEquals("a+b.txt", sharedName("inline; filename*=UTF-8''a+b.txt"), "a plus is a plus, not a space")
        assertEquals("ünï.md", sharedName("INLINE; FILENAME*=utf-8''%C3%BCn%C3%AF.md"))
        assertEquals("passwd", sharedName("inline; filename*=UTF-8''..%2F..%2Fetc%2Fpasswd"))
        assertEquals("x.sh", sharedName("inline; filename*=UTF-8''..%5Cx.sh"))
        assertEquals("profile", sharedName("inline; filename*=UTF-8''.profile"))
        for (none in listOf(null, "", "inline", "inline; filename=plain.txt", "inline; filename*=UTF-8''%ZZ", "inline; filename*=UTF-8''..")) {
            assertEquals("file", sharedName(none), "$none")
        }
        assertEquals(255, sharedName("inline; filename*=UTF-8''${"a".repeat(300)}").length)
        val wide = sharedName("inline; filename*=UTF-8''${"%E7%95%8C".repeat(100)}")
        assertEquals("界".repeat(85), wide, "cut by bytes, three to each of these")
        val faces = sharedName("inline; filename*=UTF-8''a${"%F0%9F%98%80".repeat(70)}")
        assertEquals("a" + "😀".repeat(63), faces, "a character is never cut in two")
    }

    private fun request(url: String, gesture: Boolean = true, redirect: Boolean = false, main: Boolean = true) =
        object : WebResourceRequest {
            override fun getUrl() = android.net.Uri.parse(url)

            override fun isForMainFrame() = main

            override fun isRedirect() = redirect

            override fun hasGesture() = gesture

            override fun getMethod() = "GET"

            override fun getRequestHeaders() = emptyMap<String, String>()
        }

    @Test
    fun aPageLeavesForTheBrowserOnlyWhenTheOwnerTapsAWebLink() {
        val opened = mutableListOf<String>()
        val page = SharedPage(object : UriHandler { override fun openUri(uri: String) { opened += uri } }) {}
        val view = WebView(ApplicationProvider.getApplicationContext())
        assertTrue(page.shouldOverrideUrlLoading(view, request("https://example.com/a")))
        assertTrue(page.shouldOverrideUrlLoading(view, request("https://example.com/script", gesture = false)), "a script moves nothing")
        assertTrue(page.shouldOverrideUrlLoading(view, request("https://example.com/redirect", redirect = true)), "nor does a redirect")
        assertTrue(page.shouldOverrideUrlLoading(view, request("intent://x#Intent;end")))
        assertEquals(false, page.shouldOverrideUrlLoading(view, request("https://example.com/frame", main = false)), "a frame inside the page stays in it")
        assertEquals(listOf("https://example.com/a"), opened)
    }

    @Test
    fun aGenericFileOpensAsItsExtensionSays() {
        val docx = "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        assertEquals(docx, MimeTypeMap.getSingleton().getMimeTypeFromExtension("docx"), "the phone knows the type")
        assertEquals(docx, mediaType("application/octet-stream", "Notes.DOCX"))
        assertEquals(docx, mediaType(null, "notes.docx"))
        assertEquals("application/pdf", mediaType("application/pdf", "notes.docx"), "Renoa's own type stands")
        assertEquals("application/octet-stream", mediaType("application/octet-stream", "notes"))
    }

    @Test
    fun aBodyShorterThanItSaysKeepsNothing() = runTest {
        server.enqueue(MockResponse.Builder().setHeader("Content-Disposition", "inline; filename*=UTF-8''cut.bin").body("short").build())
        val lying =
            object : ResponseBody() {
                override fun contentType(): MediaType? = null

                override fun contentLength() = 100L

                override fun source(): BufferedSource = Buffer().writeUtf8("short")
            }
        val http = OkHttpClient.Builder().addInterceptor { chain -> chain.proceed(chain.request()).newBuilder().body(lying).build() }.build()
        assertFailsWith<IOException> { fetchShared(http, server.url("/s/x").toString(), dir()) }
        assertTrue(dir().list().isNullOrEmpty(), "no part of a file is kept: ${dir().list()?.toList()}")
    }

    @Test
    fun aFetchedFileReplacesTheOneBeforeAndKeepsItsTypeWithoutParameters() = runTest {
        server.enqueue(MockResponse.Builder().setHeader("Content-Type", "application/pdf").setHeader("Content-Disposition", "inline; filename*=UTF-8''one.pdf").body("first").build())
        server.enqueue(MockResponse.Builder().setHeader("Content-Type", "text/csv; charset=utf-8").setHeader("Content-Disposition", "inline; filename*=UTF-8''two.csv").body("a,b").build())
        val http = OkHttpClient()
        val first = fetchShared(http, server.url("/s/x?ticket=1").toString(), dir())
        assertEquals("one.pdf" to "application/pdf", first.file.name to first.type)
        assertEquals("first", first.file.readText())
        val second = fetchShared(http, server.url("/s/x?ticket=2").toString(), dir())
        assertEquals("two.csv" to "text/csv", second.file.name to second.type)
        assertEquals(listOf("two.csv"), dir().list()?.toList(), "one file at a time")
    }

    @Test
    fun aRefusalOrAFileCutShortKeepsNothing() = runTest {
        val http = OkHttpClient()
        dir().mkdirs()
        File(dir(), "kept.pdf").writeText("kept")
        server.enqueue(MockResponse.Builder().code(404).body("gone").build())
        assertFailsWith<IOException> { fetchShared(http, server.url("/s/x").toString(), dir()) }
        assertEquals(listOf("kept.pdf"), dir().list()?.toList(), "a refusal changes nothing")

        server.enqueue(MockResponse.Builder().setHeader("Content-Disposition", "inline; filename*=UTF-8''cut.bin").body("short").setHeader("Content-Length", 100).onResponseEnd(SocketEffect.CloseSocket()).build())
        assertFailsWith<IOException> { fetchShared(http, server.url("/s/x").toString(), dir()) }
        assertTrue(dir().list().isNullOrEmpty(), "no part of a file is kept: ${dir().list()?.toList()}")
    }
}

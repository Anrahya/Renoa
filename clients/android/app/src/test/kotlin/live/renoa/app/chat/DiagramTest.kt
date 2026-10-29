package live.renoa.app.chat

import android.view.View
import android.view.ViewGroup
import android.webkit.RenderProcessGoneDetail
import android.webkit.WebSettings
import android.webkit.WebView
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.assertHeightIsEqualTo
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.onAllNodesWithContentDescription
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.ChatMessage
import live.renoa.app.agents.Conversation
import live.renoa.app.agents.FakeAgentRepository
import live.renoa.app.agents.Speaker
import live.renoa.app.agents.StreamingReply
import live.renoa.app.theme.RenoaTheme
import live.renoa.app.theme.ThemeName
import live.renoa.app.ui.ScreenTest
import org.junit.After
import org.junit.Test
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertSame
import kotlin.test.assertTrue

/**
 * A Mermaid fence in an agent's reply, through the chat as the owner sees it. Robolectric's web
 * view runs no script, so the page's word is given to the view as the page would give it: its title.
 */
class DiagramTest : ScreenTest() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)

    @After
    fun tearDown() = scope.coroutineContext[kotlinx.coroutines.Job]!!.cancel()

    private var theme by mutableStateOf(ThemeName.Graphite)
    private var open by mutableStateOf(true)

    /** Optimus's chat as [talk] holds it, in [theme], which a test may change while it shows, and only while [open]. */
    private fun chat(talk: MutableStateFlow<Conversation>) {
        val repository = object : AgentRepository by FakeAgentRepository(scope) { override fun conversation(agentId: String) = talk }
        val model = ChatViewModel(repository, "optimus")
        compose.setContent { RenoaTheme(theme, reducedMotion = true, clock = remember { mutableFloatStateOf(0f) }) { if (open) ChatScreen(model, onBack = {}, onOpenProfile = {}) } }
    }

    private fun reply(text: String) = MutableStateFlow(Conversation(listOf(ChatMessage("a", Speaker.Agent, text))))

    /** Closes the chat and opens it again, as a reply scrolled away and back is composed afresh. */
    private fun reopen() {
        compose.runOnUiThread { open = false }
        compose.waitForIdle()
        compose.runOnUiThread { open = true }
        compose.waitForIdle()
    }

    /** A diagram that could not be drawn shows [source] as code, with no view, and is still code when shown again. */
    private fun showsAsCodeFromNowOn(source: String) {
        compose.waitForIdle()
        compose.onNodeWithText(source).assertExists()
        compose.onAllNodes(diagram).assertCountEquals(0)
        assertEquals(emptyList(), webViews(), "the view is gone with the diagram")
        reopen()
        compose.onNodeWithText(source).assertExists()
        assertEquals(emptyList(), webViews(), "no view is made again for a diagram that could not be drawn")
    }

    private fun webViews(): List<WebView> {
        fun View.all(): List<View> = listOf(this) + ((this as? ViewGroup)?.let { group -> (0 until group.childCount).flatMap { group.getChildAt(it).all() } } ?: emptyList())
        compose.waitForIdle()
        return compose.activity.window.decorView.all().filterIsInstance<WebView>()
    }

    /** The one web view on screen, once the page's makings have been read. */
    private fun theWebView(): WebView {
        compose.waitUntil(5_000) { webViews().isNotEmpty() }
        return webViews().single()
    }

    private fun WebView.says(title: String) = compose.runOnUiThread { shadowOf(this).webChromeClient.onReceivedTitle(this, title) }

    private fun WebView.page(): String = shadowOf(this).lastLoadDataWithBaseURL.data

    private val diagram = SemanticsMatcher.expectValue(SemanticsProperties.Role, Role.Image)

    @Test
    fun aMermaidFenceInAReplyIsADiagramInALockedDownViewSizedToItsDrawing() {
        chat(MutableStateFlow(Conversation(listOf(ChatMessage("a", Speaker.Agent, DINNER)))))
        val view = theWebView()
        compose.onNodeWithContentDescription("Diagram: Dinner tonight").assertExists()
        compose.onAllNodes(diagram).assertCountEquals(1)
        // Neither the fence nor its source shows as text; the words around it do.
        compose.onAllNodes(hasText("flowchart", substring = true)).assertCountEquals(0)
        compose.onAllNodes(hasText("Check the fridge", substring = true)).assertCountEquals(0)
        compose.onNodeWithText("Here is how dinner comes together:").assertExists()
        compose.onNodeWithText("Start by seven.").assertExists()

        // The page is the diagram's, and nothing in the view reaches past it.
        val shadow = shadowOf(view)
        assertEquals(null, shadow.lastLoadDataWithBaseURL.baseUrl)
        assertTrue(view.page().contains("A[Check the fridge] --&gt; B{Lentils left?}"))
        assertTrue(view.page().contains("--pele-bg: #111312;"))
        with(view.settings) {
            assertTrue(javaScriptEnabled)
            assertTrue(blockNetworkLoads)
            assertFalse(allowFileAccess)
            assertFalse(allowContentAccess)
            assertFalse(javaScriptCanOpenWindowsAutomatically)
            assertFalse(supportMultipleWindows())
            assertFalse(supportZoom())
            assertFalse(builtInZoomControls)
        }
        assertFalse(view.isFocusable || view.isFocusableInTouchMode, "nothing to type into")
        assertTrue(shadow.webViewClient.shouldOverrideUrlLoading(view, request("https://example.com")), "a link in the drawing goes nowhere")
        val answer = shadow.webViewClient.shouldInterceptRequest(view, request("https://example.com/cat.png"))
        assertNotNull(answer, "a request the view asks about is answered here, not passed on")
        assertNull(answer.data, "and answered with nothing")

        // Until the page has drawn, the reply holds a placeholder; then the box takes the drawing's height.
        compose.onNodeWithText(DRAWING).assertExists()
        view.says("about:blank")
        compose.onNodeWithText(DRAWING).assertExists()
        val loaded = shadow.lastLoadDataWithBaseURL
        view.says("h:240")
        compose.waitForIdle()
        compose.onAllNodesWithText(DRAWING).assertCountEquals(0)
        compose.onNodeWithContentDescription("Diagram: Dinner tonight").assertHeightIsEqualTo(240.dp)
        assertSame(loaded, shadow.lastLoadDataWithBaseURL, "taking its height does not draw the diagram again")
        snapshot("chat-diagram")

        // The theme changes: the same view draws the page again in the other theme's colours.
        compose.runOnUiThread { theme = ThemeName.Paper }
        compose.waitForIdle()
        assertSame(view, theWebView())
        assertTrue(view.page().contains("--pele-bg: #F5F6F3;"), "the page follows the theme")
    }

    @Test
    fun aDiagramPeleCannotDrawShowsItsSourceAsCode() {
        chat(reply("Broken:\n\n```mermaid\nflowchart LR\n  A -->\n```\n"))
        theWebView().says("x")
        showsAsCodeFromNowOn("flowchart LR\n  A -->")
    }

    @Test
    fun aDiagramTallerThanTenScreensShowsAsCode() {
        chat(reply("Long:\n\n```mermaid\nsequenceDiagram\n  Alice->>Bob: Hi\n```\n"))
        theWebView().says("h:55630")
        showsAsCodeFromNowOn("sequenceDiagram\n  Alice->>Bob: Hi")
    }

    @Test
    // The detail's constructor is deprecated for apps, which are only ever handed one; a test has to make it.
    @Suppress("DEPRECATION")
    fun aDiagramWhoseRendererWentShowsAsCodeAndTheAppCarriesOn() {
        chat(reply("Gone:\n\n```mermaid\nflowchart LR\n  Gone --> Code\n```\n"))
        val view = theWebView()
        val crashed = object : RenderProcessGoneDetail() {
            override fun didCrash() = true

            override fun rendererPriorityAtExit() = 0
        }
        assertTrue(compose.runOnUiThread { shadowOf(view).webViewClient.onRenderProcessGone(view, crashed) }, "the app is not taken down with it")
        showsAsCodeFromNowOn("flowchart LR\n  Gone --> Code")
    }

    @Test
    fun twoDiagramsInOneReplyKeepTheirOwnViewsAsTheReplyArrives() {
        val talk = MutableStateFlow(Conversation())
        chat(talk)
        val both = "First:\n\n```mermaid\nflowchart LR\n  One --> Two\n```\n\nThen:\n\n```mermaid\nflowchart LR\n  Three --> Four\n```\n"
        fun arrive(text: String) {
            compose.runOnUiThread { talk.update { it.copy(streaming = StreamingReply("r", text)) } }
            compose.waitForIdle()
        }
        arrive(both)
        compose.waitUntil(5_000) { webViews().size == 2 }
        fun bySource() = webViews().associateBy { view -> listOf("One --&gt; Two", "Three --&gt; Four").single { it in view.page() } }
        val first = bySource()
        for (more in listOf("$both\nThat", "$both\nThat is all.")) {
            arrive(more)
            val now = bySource()
            assertEquals(first.keys, now.keys)
            for (key in first.keys) assertSame(first.getValue(key), now.getValue(key), key)
        }
        compose.runOnUiThread { talk.update { it.copy(messages = it.messages + ChatMessage("r", Speaker.Agent, "$both\nThat is all."), streaming = null) } }
        compose.waitForIdle()
        val finished = bySource()
        for (key in first.keys) assertSame(first.getValue(key), finished.getValue(key), "$key, once the reply is whole")
    }

    @Test
    @Config(sdk = [31])
    // Force dark is deprecated from Android 13; this reads it below that, where it still applies.
    // Robolectric's setting starts at off, not a phone's auto, so this catches a wrong value but not
    // a missing call; the page's own colour-scheme claim, tested with the page, covers that.
    @Suppress("DEPRECATION")
    fun aPhoneInDarkModeBeforeAndroid13DoesNotDarkenADiagram() {
        chat(reply(DINNER))
        assertEquals(WebSettings.FORCE_DARK_OFF, theWebView().settings.forceDark)
    }

    @Test
    fun anArrivingDiagramIsAPlaceholderUntilItsFenceClosesAndKeepsOneViewFromThenOn() {
        val talk = MutableStateFlow(Conversation())
        chat(talk)
        fun arrive(text: String) {
            compose.runOnUiThread { talk.update { it.copy(streaming = StreamingReply("r", text)) } }
            compose.waitForIdle()
        }
        val open = DINNER.substringBefore("```\n\nStart")
        // Word by word up to the closing fence: a placeholder, never the source or a diagram.
        val words = open.split(" ")
        for (n in 1..words.size) {
            arrive(words.take(n).joinToString(" "))
            compose.onAllNodes(hasText("Check the fridge", substring = true)).assertCountEquals(0)
            compose.onAllNodes(diagram).assertCountEquals(0)
            assertEquals(emptyList(), webViews())
        }
        compose.onNodeWithText(DRAWING).assertExists()

        // The fence closes: the diagram is drawn, in one view that stays as the reply carries on and becomes its message.
        arrive("$open```\n")
        val view = theWebView()
        val loaded = shadowOf(view).lastLoadDataWithBaseURL
        compose.onNodeWithContentDescription("Diagram: Dinner tonight").assertExists()
        for (rest in listOf("$open```\n\nStart", "$open```\n\nStart by", DINNER)) {
            arrive(rest)
            assertSame(view, theWebView())
        }
        compose.runOnUiThread { talk.update { it.copy(messages = it.messages + ChatMessage("r", Speaker.Agent, DINNER)) } }
        compose.waitForIdle()
        compose.runOnUiThread { talk.update { it.copy(streaming = null) } }
        compose.waitForIdle()
        compose.onNodeWithText("Start by seven.").assertExists()
        assertSame(view, theWebView(), "the finished message keeps the view drawn while it arrived")
        assertSame(loaded, shadowOf(view).lastLoadDataWithBaseURL, "and never loads its page again")
        compose.onAllNodesWithContentDescription("Diagram: Dinner tonight").assertCountEquals(1)
    }

    private fun request(url: String) =
        object : android.webkit.WebResourceRequest {
            override fun getUrl() = android.net.Uri.parse(url)

            override fun isForMainFrame() = true

            override fun isRedirect() = false

            override fun hasGesture() = true

            override fun getMethod() = "GET"

            override fun getRequestHeaders() = emptyMap<String, String>()
        }
}

private const val DINNER = "Here is how dinner comes together:\n\n```mermaid\nflowchart TD\n  accTitle: Dinner tonight\n  A[Check the fridge] --> B{Lentils left?}\n  B -->|Yes| C[Dal and rice]\n  B -->|No| D[Order groceries]\n  D --> C\n```\n\nStart by seven."

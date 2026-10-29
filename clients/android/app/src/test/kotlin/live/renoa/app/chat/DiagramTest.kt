package live.renoa.app.chat

import android.view.View
import android.view.ViewGroup
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
import kotlin.test.assertEquals
import kotlin.test.assertFalse
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

    /** Optimus's chat as [talk] holds it, in [theme], which a test may change while it shows. */
    private fun chat(talk: MutableStateFlow<Conversation>) {
        val repository = object : AgentRepository by FakeAgentRepository(scope) { override fun conversation(agentId: String) = talk }
        val model = ChatViewModel(repository, "optimus")
        compose.setContent { RenoaTheme(theme, reducedMotion = true, clock = remember { mutableFloatStateOf(0f) }) { ChatScreen(model, onBack = {}, onOpenProfile = {}) } }
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
        }
        assertFalse(view.isFocusable || view.isFocusableInTouchMode, "nothing to type into")
        assertTrue(shadow.webViewClient.shouldOverrideUrlLoading(view, request("https://example.com")), "a link in the drawing goes nowhere")

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
        chat(MutableStateFlow(Conversation(listOf(ChatMessage("a", Speaker.Agent, "Broken:\n\n```mermaid\nflowchart LR\n  A -->\n```\n")))))
        theWebView().says("x")
        compose.waitForIdle()
        compose.onNodeWithText("flowchart LR\n  A -->").assertExists()
        compose.onAllNodes(diagram).assertCountEquals(0)
        assertEquals(emptyList(), webViews(), "the view is gone with the diagram")
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

package live.renoa.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.background
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.assert
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasScrollToNodeAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.filter
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.hasAnyAncestor
import androidx.compose.ui.test.onAllNodesWithContentDescription
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollToNode
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.ChatMessage
import live.renoa.app.agents.Conversation
import live.renoa.app.agents.Speaker
import kotlinx.coroutines.flow.flowOf
import live.renoa.app.agents.AgentProfile
import live.renoa.app.agents.EditProfile
import live.renoa.app.agents.Problem
import live.renoa.app.agents.RepositoryException
import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.performTextReplacement
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.badge.AgentHue
import live.renoa.app.badge.BadgeFrame.Companion.LIFT
import live.renoa.app.badge.BadgeFrame.Companion.RADIUS
import live.renoa.app.badge.HALF
import live.renoa.app.badge.readSvg
import androidx.compose.ui.geometry.Offset
import java.io.File
import kotlin.math.roundToInt
import live.renoa.app.agents.FakeAgentRepository
import live.renoa.app.badge.Badge
import live.renoa.app.badge.PORTRAIT_GROUND
import androidx.compose.ui.graphics.toArgb
import live.renoa.app.chat.ChatScreen
import android.content.Context
import android.net.wifi.WifiManager
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows.shadowOf
import live.renoa.app.chat.ChatViewModel
import live.renoa.app.editor.AgentEditorScreen
import live.renoa.app.editor.AgentEditorViewModel
import live.renoa.app.editor.EditorMode
import live.renoa.app.home.HomeScreen
import live.renoa.app.home.OPEN_PAGES
import live.renoa.app.home.HomeViewModel
import live.renoa.app.signin.SignInScreen
import live.renoa.app.signup.SignUpMemory
import live.renoa.app.signup.SignUpScreen
import live.renoa.app.signup.SignUpViewModel
import live.renoa.app.signin.SignInState
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.ThemeName
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.assertIsNotFocused
import androidx.compose.ui.test.hasScrollToIndexAction
import androidx.compose.ui.test.performImeAction
import androidx.compose.ui.test.performScrollToIndex
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import live.renoa.app.agents.Link
import live.renoa.app.theme.RenoaType
import live.renoa.app.home.CHECKING_IN
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.UriHandler
import androidx.compose.ui.test.SemanticsNodeInteraction
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.isHeading
import androidx.compose.ui.test.performFirstLinkClick
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.swipe
import live.renoa.app.agents.StreamingReply
import live.renoa.app.chat.ArrivingReply
import live.renoa.app.chat.Reply
import com.mikepenz.markdown.model.State as MarkdownState
import kotlin.test.assertTrue
import org.junit.After
import org.junit.Test
import kotlin.test.assertEquals

class ScreensTest : ScreenTest() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
    private val repository by lazy { FakeAgentRepository(scope, now = { EVENING }) }

    @After
    fun tearDown() = scope.coroutineContext[kotlinx.coroutines.Job]!!.cancel()

    private fun badgesIn(theme: ThemeName) {
        show(theme, seconds = 3.97f) {
            Column(Modifier.background(Renoa.colors.background).padding(20.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    AgentState.entries.forEach { Badge("Tifa", it, 66.dp, primary = false, hue = AgentHue.Ink, label = "Tifa ${it.name}") }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    AgentState.entries.forEach { Badge("Renoa", it, 66.dp, primary = true, hue = AgentHue.Ink, label = "Renoa ${it.name}") }
                }
            }
        }
        compose.onNodeWithContentDescription("Tifa Working, working").assertIsDisplayed()
        compose.onNodeWithContentDescription("Tifa Wants, wants you").assertIsDisplayed()
        snapshot("badges-${theme.name.lowercase()}")
    }

    @Test
    fun theMainAgentWearsRenoasPortraitInGraphite() = portraitIn(ThemeName.Graphite)

    @Test
    fun theMainAgentWearsRenoasPortraitInPaper() = portraitIn(ThemeName.Paper)

    /** Her eyes land where her portrait rect puts them on the main agent's tile, and another agent keeps its letter. */
    private fun portraitIn(theme: ThemeName) {
        val roster = listOf(AgentSummary("renoa", "Renoa", "", AgentState.Resting, primary = true), AgentSummary("tifa", "Tifa", "", AgentState.Resting))
        show(theme) {
            Column(Modifier.background(Renoa.colors.background).padding(20.dp), verticalArrangement = Arrangement.spacedBy(40.dp)) {
                roster.forEach { Badge(it, roster, 140.dp) }
            }
        }
        val picture = picture()
        val dp = picture.density / 160f
        // Where the tile's centre is drawn, and how many pixels one unit of the drawing spans.
        val perTileUnit = 140 * dp / (HALF * 2)
        val centre = Offset(20 * dp + 70 * dp, 20 * dp + 70 * dp + LIFT * 0.3f * perTileUnit)
        val crop = readSvg(File("src/main/res/raw/renoa_mascot.svg").readText()).portrait
        val perUnit = perTileUnit * RADIUS * 2 / crop.width
        fun at(x: Float, y: Float) = picture.getPixel((centre.x + (x - crop.center.x) * perUnit).roundToInt(), (centre.y + (y - crop.center.y) * perUnit).roundToInt())
        val iris = 0xFF9A3B2C.toInt()
        assertEquals(iris, at(82f, 111f), "her left eye")
        assertEquals(iris, at(118f, 111f), "her right eye")

        val skin = 0xFFF6D9C4.toInt()
        val below = ((20 + 140 + 20) * dp).roundToInt()
        assertTrue((0 until picture.width step 2).none { x -> (below until picture.height step 2).any { y -> picture.getPixel(x, y) == skin } }, "another agent keeps its letter")
        snapshot("badge-portrait-${theme.name.lowercase()}")
    }

    @Test
    fun meetingRenoaInGraphite() {
        val fresh = FakeAgentRepository(scope, seeds = emptyList())
        kotlinx.coroutines.runBlocking { fresh.saveOwnerProfile(fresh.ownerProfile().revision, "Name: Yash\nA usual day: Busy\n") }
        val model = SignUpViewModel(fresh, object : SignUpMemory { override var operationId: String? = null })
        // Reduced motion: she is there at once, not partway through dropping in.
        show(ThemeName.Graphite, reducedMotion = true) { SignUpScreen(model, onDone = {}) }
        compose.waitForIdle()
        compose.onNodeWithText("I'm Renoa.").assertIsDisplayed()
        // Past the step's own fade-in, so the picture is of the screen at rest.
        compose.mainClock.advanceTimeBy(1_000)
        compose.waitForIdle()
        // Her dark hair would vanish on Graphite's ground, so she stands on Paper's, as in her tile.
        val figure = compose.onNodeWithContentDescription("Renoa").fetchSemanticsNode().boundsInRoot
        assertEquals(PORTRAIT_GROUND.toArgb(), picture().getPixel(figure.left.toInt() + 3, figure.top.toInt() + 3))
        snapshot("sign-up-3-graphite")
    }

    @Test
    fun theFiveBadgeStatesSideBySideInGraphite() = badgesIn(ThemeName.Graphite)

    @Test
    fun theFiveBadgeStatesSideBySideInPaper() = badgesIn(ThemeName.Paper)

    private fun home(theme: ThemeName = ThemeName.Graphite, seconds: Float = 0f, onOpenChat: (String) -> Unit = {}, onNewAgent: () -> Unit = {}, onChangeColours: () -> Unit = {}) {
        val model = HomeViewModel(repository)
        show(theme, seconds) { HomeScreen(model, onOpenChat, onNewAgent, onChangeColours, now = { EVENING }) }
        compose.waitForIdle()
    }

    private fun openPages() {
        compose.onNode(hasClickLabel(OPEN_PAGES)).performClick()
        compose.waitForIdle()
    }

    @Test
    fun homeShowsTheDayRenoaWithEveryoneAndTheOneThingThatNeedsTheOwner() {
        home(seconds = 3.97f)
        compose.onNodeWithText("Good evening.").assertIsDisplayed()
        compose.onNodeWithText("THU 7:40 PM").assertIsDisplayed()
        compose.onNodeWithContentDescription("Today. Morning brief ran at 7:30 AM. GitHub check ran at 6:30 PM. Prices and groceries ran at 7:15 PM. Posts worth a reply at 8 PM. Evening check-in at 9 PM").assertIsDisplayed()
        compose.onNodeWithContentDescription("Renoa, working, your main agent").assertIsDisplayed()
        compose.onNodeWithContentDescription("Tifa, wants you").assertIsDisplayed()
        compose.onNodeWithContentDescription("3 more, in Chats").assertIsDisplayed()
        compose.onNodeWithText("Renoa and Kai'Sa · Picking three posts worth your reply…").assertIsDisplayed()
        compose.onNodeWithText("What's the plan for dinner? You were light on fibre at lunch.").assertIsDisplayed()
        compose.onNodeWithText("1 of 6").assertIsDisplayed()
        compose
            .onNode(hasClickLabel(OPEN_PAGES))
            .assert(hasText("New from Tifa and Optimus"))
            .assert(hasText("Next: Posts worth a reply at 8 PM"))
            .assert(SemanticsMatcher.keyNotDefined(SemanticsProperties.ContentDescription))
        compose.onNodeWithText("Next: Posts worth a reply at 8 PM").assertIsDisplayed()
        snapshot("home")
    }

    @Test
    fun theOneThingStepsThroughTheRestAndAQuestionIsAnsweredInPlace() {
        home()
        compose.onNodeWithContentDescription("Previous").assertIsNotEnabled()
        compose.onNodeWithContentDescription("Next").performClick()
        compose.waitForIdle()
        compose.onNodeWithText("I couldn't open last month's statement. Can you send it again?").assertIsDisplayed()
        compose.onNodeWithText("2 of 6").assertIsDisplayed()
        compose.onNodeWithContentDescription("Previous").performClick()
        compose.waitForIdle()

        compose.onNodeWithText("Dal and a salad").performClick()
        compose.waitForIdle()
        compose.onNodeWithText("Dal and a salad").assertDoesNotExist()
        compose.onNodeWithText("I couldn't open last month's statement. Can you send it again?").assertIsDisplayed()
    }

    @Test
    fun tappingSomeoneOpensTheirChatAndTheRestAreInChats() {
        val opened = mutableListOf<String>()
        var made = 0
        home(onOpenChat = { opened += it }, onNewAgent = { made++ })
        compose.onNodeWithContentDescription("Optimus, resting").performClick()
        compose.onNodeWithContentDescription("Renoa, working, your main agent").performClick()
        compose.onNodeWithText("What's the plan for dinner? You were light on fibre at lunch.").performClick()
        assertEquals(listOf("optimus", "renoa", "tifa"), opened)

        compose.onNodeWithContentDescription("3 more, in Chats").performClick()
        compose.waitForIdle()
        val chats = compose.onAllNodes(hasScrollToNodeAction()).filter(hasAnyAncestor(hasScrollToNodeAction())).onFirst()
        chats.performScrollToNode(hasContentDescription("Echo, asleep"))
        compose.onNodeWithContentDescription("Echo, asleep").performClick()
        chats.performScrollToNode(hasContentDescription("Meet someone new"))
        compose.onNodeWithContentDescription("Meet someone new").performClick()
        assertEquals("echo", opened.last())
        assertEquals(1, made)

        compose.onNodeWithContentDescription("Back to Home").performClick()
        compose.waitForIdle()
        compose.onNodeWithContentDescription("Optimus, resting").assertIsDisplayed()
    }

    @Test
    fun aRoutineIsTurnedOnFromItsPageAndWhatIsComingUpIsListed() {
        home()
        openPages()
        compose.onNodeWithText("Routines").performClick()
        compose.waitForIdle()
        compose.onNodeWithText("Tifa · Every day at 1 PM").assertIsDisplayed()
        compose.onNodeWithText("Paused").assertIsDisplayed()
        compose.onNodeWithContentDescription("Lunch photo").performClick()
        compose.waitForIdle()
        assertTrue(repository.routines.value.first { it.id == "lunch" }.on)
        compose.onNodeWithText("Next: tomorrow at 1 PM").assertIsDisplayed()

        compose.onNodeWithText("Upcoming").performClick()
        compose.waitForIdle()
        compose.onNodeWithText("LATER TODAY").assertIsDisplayed()
        compose.onNodeWithText("Posts worth a reply").assertIsDisplayed()
        compose.onNodeWithText("TOMORROW").assertIsDisplayed()
        snapshot("home-upcoming")

        compose.activity.onBackPressedDispatcher.onBackPressed()
        compose.waitForIdle()
        compose.onNodeWithContentDescription("Tifa, wants you").assertIsDisplayed()
        compose.onNodeWithText("Upcoming").assertDoesNotExist()
    }

    @Test
    fun homeInPaper() {
        home(ThemeName.Paper, seconds = 3.97f)
        compose.onNodeWithText("What's the plan for dinner? You were light on fibre at lunch.").assertIsDisplayed()
        snapshot("home-paper")
        openPages()
        snapshot("home-chats-paper")
    }

    @Test
    fun beforeRenoaFirstAnswersHomeWaitsQuietlyRatherThanLookingEmpty() {
        val known = MutableStateFlow(false)
        val waiting =
            object : AgentRepository by FakeAgentRepository(scope, seeds = emptyList(), routineSeeds = emptyList()) {
                override val rosterKnown = known
                override val link = MutableStateFlow(Link.Connecting)

                override suspend fun hasAgents(): Boolean = awaitCancellation()
            }
        val model = HomeViewModel(waiting)
        show { HomeScreen(model, onOpenChat = {}, onNewAgent = {}, onChangeColours = {}, now = { EVENING }) }
        compose.waitForIdle()
        compose.onNodeWithText(CHECKING_IN).assertIsDisplayed()
        compose.onNodeWithContentDescription("Today.").assertIsDisplayed()
        compose.onNodeWithContentDescription("Make your first agent").assertDoesNotExist()
        compose.onNodeWithText("Nobody is here yet.").assertDoesNotExist()
        compose.onNode(hasClickLabel(OPEN_PAGES)).assertDoesNotExist()
        snapshot("home-connecting")

        // Once Renoa says no one is here, Home says so and offers to make someone.
        known.value = true
        compose.waitForIdle()
        compose.onNodeWithContentDescription("Today. Nothing runs today.").assertIsDisplayed()
        compose.onNodeWithContentDescription("Make your first agent").assertIsDisplayed()
        compose.onNodeWithText(CHECKING_IN).assertDoesNotExist()
    }

    @Test
    fun theColourChangeLivesOnTheMark() {
        var changed = 0
        home(onChangeColours = { changed++ })
        compose.onNodeWithContentDescription("Renoa, change colours").performClick()
        assertEquals(1, changed)
    }

    @Test
    fun aWorkingAgentSaysWhatItIsDoingAndCanBeStopped() {
        val model = ChatViewModel(repository, "renoa")
        show(seconds = 3f) { ChatScreen(model, onBack = {}, onOpenProfile = {}) }
        compose.waitForIdle()
        compose.onNodeWithText("Can you and Kai'Sa find me three posts I could reply to tonight?").assertIsDisplayed()
        compose.onNodeWithText("The first one is a thread about running AI on your own phone. Two more coming.").assertIsDisplayed()
        compose.onNodeWithText("Picking three posts worth your reply…").assertIsDisplayed()
        compose.onNodeWithContentDescription("Stop").assertIsEnabled()
        snapshot("chat-working")
    }

    /** Optimus's chat as [talk] holds it. What the owner sends is listed in [sent]; it joins the chat only when the test adds it. */
    private fun chatWith(talk: MutableStateFlow<Conversation>, sent: MutableList<String> = mutableListOf()) =
        object : AgentRepository by repository {
            override fun conversation(agentId: String) = talk

            override suspend fun send(agentId: String, text: String) {
                sent += text
            }
        }

    /** A conversation far longer than the screen, oldest first. */
    private fun longChat() =
        List(30) { i -> if (i % 2 == 0) ChatMessage("m$i", Speaker.Owner, "Question $i about the week ahead?") else ChatMessage("m$i", Speaker.Agent, "Answer $i. Here is what I found for you, in a few lines.") }

    private fun agentSays(talk: MutableStateFlow<Conversation>, id: String, text: String) =
        compose.runOnUiThread { talk.update { it.copy(messages = it.messages + ChatMessage(id, Speaker.Agent, text)) } }

    @Test
    fun anAgentsReplyShowsItsMarkdownAndOnlyItsWebLinksAreLinks() {
        val opened = mutableListOf<String>()
        val browser = object : UriHandler { override fun openUri(uri: String) { opened += uri } }
        val owner = "What's for dinner? **Something** quick."
        val talk = MutableStateFlow(Conversation(listOf(ChatMessage("q", Speaker.Owner, owner), ChatMessage("a", Speaker.Agent, MARKDOWN_REPLY))))
        val model = ChatViewModel(chatWith(talk), "optimus")
        show { CompositionLocalProvider(LocalUriHandler provides browser) { ChatScreen(model, onBack = {}, onOpenProfile = {}) } }
        compose.waitForIdle()

        // The owner's words stay as typed; the reply shows none of its markdown syntax.
        compose.onNodeWithText(owner).assertIsDisplayed()
        for (syntax in listOf("**", "##", "```", "](", "![")) compose.onAllNodes(hasText(syntax, substring = true)).filter(!hasText(owner)).assertCountEquals(0)
        compose.onNode(hasText("Dinner plan") and isHeading()).assertIsDisplayed()
        val intro = textOf("Here is what I found for tonight:")
        assertTrue(intro.spanStyles.any { it.item.fontWeight == FontWeight.Bold && intro.text.substring(it.start, it.end) == "what I found" }, "bold")
        compose.onAllNodes(hasText("•", substring = true)).assertCountEquals(2)
        val dal = textOf(compose.onNode(hasText("brown rice", substring = true)))
        val muted = ThemeName.Graphite.colors.muted
        assertTrue(dal.spanStyles.any { it.item.background == muted && it.item.fontFamily == RenoaType.Code.fontFamily && dal.text.substring(it.start, it.end).trim() == "brown rice" }, "inline code")

        // A code block scrolls sideways rather than wrapping, and its language puts no bar over it.
        val code = "val cups = mapOf(\"rice\" to 1, \"dal\" to 2, \"salt\" to 0, \"water\" to 4, \"ghee\" to 1)"
        val scrolls = SemanticsMatcher.keyIsDefined(SemanticsProperties.HorizontalScrollAxisRange)
        compose.onNode(hasText(code) and (scrolls or hasAnyAncestor(scrolls))).assertIsDisplayed()
        for (bar in listOf("kotlin", "KOTLIN", "CODE", "Copy code")) {
            compose.onAllNodes(hasText(bar) or hasContentDescription(bar)).assertCountEquals(0)
        }
        compose.onAllNodes(hasText("Language", substring = true)).assertCountEquals(0)
        // An image is its description: a reply loads nothing.
        compose.onNodeWithText("A bowl of dal").assertIsDisplayed()

        val web = compose.onNode(hasText("Recipe at", substring = true))
        val links = textOf(web).getLinkAnnotations(0, textOf(web).length)
        assertEquals(listOf("https://example.com/dal", "HTTP://example.com/up", "www.example.com"), links.map { (it.item as LinkAnnotation.Url).url })
        assertTrue(links.all { it.item.styles?.style?.textDecoration == TextDecoration.Underline }, "links look like links")
        for (link in links) web.performFirstLinkClick { it == link }
        compose.waitForIdle()
        assertEquals(listOf("https://example.com/dal", "http://example.com/up", "https://www.example.com"), opened)

        // Anything else is its words alone: nothing that looks tappable, nothing that is.
        val other = textOf(compose.onNode(hasText("Not for here:", substring = true)))
        assertEquals("Not for here: call them, run, a file, an app, https://x.com or me@example.com.", other.text)
        assertEquals(emptyList(), other.getLinkAnnotations(0, other.length))
        assertTrue(other.spanStyles.none { it.item.textDecoration == TextDecoration.Underline }, "no link styling")
        snapshot("chat-markdown")
    }

    @Test
    fun aReplyStillArrivingDrawsEachKindOfBlockAsItArrivesAndNeverBlanks() {
        var text by mutableStateOf(UNFINISHED_REPLY.take(1))
        compose.mainClock.autoAdvance = false
        show { ArrivingReply(text, Modifier.testTag("reply")) }
        compose.mainClock.advanceTimeByFrame()
        val inReply = hasAnyAncestor(hasTestTag("reply"))
        // Its first frame already shows what has arrived, before the stream has parsed any of it.
        compose.onNode(hasText("H") and inReply, useUnmergedTree = true).assertExists()
        // Each block, by the source that completes it and the text it then shows.
        val blocks =
            listOf(
                "**Dal**" to hasText("Here is the plan. Dal", substring = true),
                "## Shopping\n" to (hasText("Shopping") and isHeading()),
                "1. Lentils\n" to hasText("Lentils"),
                "- Basmati\n" to hasText("Basmati"),
                "  - Jasmine\n" to hasText("Jasmine"),
                "| Quinoa | 1 |\n" to hasText("Quinoa", substring = true),
                "> Soak overnight.\n" to hasText("Soak overnight."),
                "listOf(\"dal\"" to hasText("val shopping = listOf(\"dal\"", substring = true),
            )
        for (end in 2..UNFINISHED_REPLY.length + 1) {
            if (end <= UNFINISHED_REPLY.length) compose.runOnUiThread { text = UNFINISHED_REPLY.take(end) }
            compose.mainClock.advanceTimeByFrame()
            // A piece shows by the frame after it arrives, and stays: nothing blanks while the rest is parsed.
            val shown = UNFINISHED_REPLY.take(end - 1)
            for ((source, block) in blocks) {
                if (source in shown) compose.onNode(block and inReply, useUnmergedTree = true).assertExists("$source, with ${shown.length} characters in")
            }
            compose.onNode(hasText("H", substring = true) and inReply, useUnmergedTree = true).assertExists()
        }
    }

    @Test
    fun anOpenChatKeepsWifiQuickAndLetsGoWhenItCloses() {
        val wifi = shadowOf(RuntimeEnvironment.getApplication().getSystemService(Context.WIFI_SERVICE) as WifiManager)
        val model = ChatViewModel(repository, "renoa")
        var open by mutableStateOf(true)
        show { if (open) ChatScreen(model, onBack = {}, onOpenProfile = {}) }
        compose.waitForIdle()
        assertEquals(1, wifi.activeLockCount)
        open = false
        compose.waitForIdle()
        assertEquals(0, wifi.activeLockCount)
    }

    @Test
    fun aReplyTheAgentStartsAgainShowsInEveryFrameAndThenShowsTheNewText() {
        var text by mutableStateOf("abc")
        compose.mainClock.autoAdvance = false
        show { ArrivingReply(text, Modifier.testTag("reply")) }
        compose.mainClock.advanceTimeByFrame()
        val drawn = hasAnyAncestor(hasTestTag("reply")) and SemanticsMatcher.keyIsDefined(SemanticsProperties.Text)
        for (more in listOf("abc and", "abc and some", "abc and some more words")) {
            compose.runOnUiThread { text = more }
            compose.mainClock.advanceTimeByFrame()
        }
        // The agent starts again: the new text does not carry on from the old, with no empty text between.
        compose.runOnUiThread { text = "x" }
        repeat(4) {
            compose.mainClock.advanceTimeByFrame()
            assertTrue(compose.onAllNodes(drawn, useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty(), "frame $it after the restart is blank")
        }
        compose.onNode(hasText("x") and hasAnyAncestor(hasTestTag("reply")), useUnmergedTree = true).assertExists()
        compose.onAllNodes(hasText("abc", substring = true), useUnmergedTree = true).assertCountEquals(0)
    }

    @Test
    fun aReplyTheParserCannotReadShowsAsItsPlainText() {
        show { Reply("**Still** here", MarkdownState.Error(StackOverflowError())) }
        compose.onNodeWithText("**Still** here").assertIsDisplayed()
    }

    @Test
    fun aReplyArrivingBecomesItsMessageWithNoBlankFrameAndNoSecondCopy() {
        val talk = MutableStateFlow(Conversation(listOf(ChatMessage("q", Speaker.Owner, "Plan dinner?"))))
        val model = ChatViewModel(chatWith(talk), "optimus")
        compose.mainClock.autoAdvance = false
        show { ChatScreen(model, onBack = {}, onOpenProfile = {}) }
        repeat(3) {
            compose.waitForIdle()
            compose.mainClock.advanceTimeByFrame()
        }
        val whole = "Here is the plan: **dal** with rice, and a salad on the side."
        val words = whole.split(" ")
        val start = hasText("Here is", substring = true)
        fun frame(change: (Conversation) -> Conversation) {
            compose.runOnUiThread { talk.update(change) }
            compose.waitForIdle()
            compose.mainClock.advanceTimeByFrame()
        }
        for (n in 1..words.size) {
            frame { it.copy(streaming = StreamingReply("r1", words.take(n).joinToString(" "))) }
            if (n > 2) compose.onAllNodes(start, useUnmergedTree = true).assertCountEquals(1)
        }
        // The message arrives before the reply in progress is cleared, then the reply is cleared.
        frame { it.copy(messages = it.messages + ChatMessage("r1", Speaker.Agent, whole)) }
        compose.onAllNodes(start, useUnmergedTree = true).assertCountEquals(1)
        frame { it.copy(streaming = null) }
        repeat(3) {
            compose.onAllNodes(start, useUnmergedTree = true).assertCountEquals(1)
            compose.mainClock.advanceTimeByFrame()
        }
        compose.onNodeWithText("Here is the plan: dal with rice, and a salad on the side.").assertIsDisplayed()
    }

    /** Selectable replies would catch these drags: a SelectionContainer around a reply fails this test. */
    @Test
    fun aDragThatStartsOnAReplyScrollsTheChat() {
        val talk = MutableStateFlow(Conversation(longChat()))
        val model = ChatViewModel(chatWith(talk), "optimus")
        show { ChatScreen(model, onBack = {}, onOpenProfile = {}) }
        compose.waitForIdle()
        compose.onNodeWithText("Question 0 about the week ahead?").assertDoesNotExist()
        // Each drag starts on an agent's reply, the text a long press would select.
        repeat(8) {
            compose.onAllNodes(hasText("Here is what I found for you", substring = true)).onFirst().performTouchInput { swipe(center, center + Offset(0f, 1200f)) }
            compose.waitForIdle()
        }
        compose.waitForIdle()
        compose.onNodeWithText("Question 0 about the week ahead?").assertIsDisplayed()
    }

    private fun textOf(text: String) = textOf(compose.onNodeWithText(text))

    private fun textOf(node: SemanticsNodeInteraction) = node.fetchSemanticsNode().config[SemanticsProperties.Text].single()

    @Test
    fun theKeyboardsSendSendsPutsTheKeyboardAwayAndBringsTheMessageIntoView() {
        val talk = MutableStateFlow(Conversation(longChat()))
        val sent = mutableListOf<String>()
        val model = ChatViewModel(chatWith(talk, sent), "optimus")
        show { ChatScreen(model, onBack = {}, onOpenProfile = {}) }
        compose.waitForIdle()
        val field = compose.onNodeWithContentDescription("Message Optimus")
        field.performImeAction()
        compose.waitForIdle()
        assertEquals(emptyList(), sent, "nothing typed, nothing sent")
        field.assertIsFocused()

        compose.onNode(hasScrollToIndexAction()).performScrollToIndex(29)
        compose.onNodeWithText("Answer 29. Here is what I found for you, in a few lines.").assertDoesNotExist()
        field.performTextInput("Book the table for eight")
        field.assertIsFocused()
        field.performImeAction()
        compose.waitForIdle()
        assertEquals(listOf("Book the table for eight"), sent)
        field.assertIsNotFocused()
        // The message shows once the repository has it, a moment after the send, and the list goes to it.
        compose.runOnUiThread { talk.update { it.copy(messages = it.messages + ChatMessage("sent", Speaker.Owner, "Book the table for eight")) } }
        compose.waitForIdle()
        compose.onNodeWithText("Book the table for eight").assertIsDisplayed()

        // The newest message keeps a gap above the composer.
        val list = compose.onNode(hasScrollToIndexAction()).fetchSemanticsNode().boundsInRoot
        val newest = compose.onNodeWithText("Book the table for eight").fetchSemanticsNode().boundsInRoot
        assertTrue(list.bottom - newest.bottom >= with(compose.density) { 16.dp.toPx() } - 0.5f, "${list.bottom - newest.bottom} px under the newest message")
        snapshot("chat-sent")
    }

    @Test
    fun aReplyArrivingAtTheBottomIsFollowedAndOneArrivingWhileReadingWaitsBelow() {
        val talk = MutableStateFlow(Conversation(longChat()))
        val model = ChatViewModel(chatWith(talk), "optimus")
        show { ChatScreen(model, onBack = {}, onOpenProfile = {}) }
        compose.waitForIdle()
        agentSays(talk, "first", "Here is the first of them.")
        compose.waitForIdle()
        compose.onNodeWithText("Here is the first of them.").assertIsDisplayed()

        compose.onNode(hasScrollToIndexAction()).performScrollToIndex(30)
        compose.waitForIdle()
        agentSays(talk, "second", "And here is the second.")
        compose.waitForIdle()
        compose.onNodeWithText("And here is the second.").assertDoesNotExist()
        compose.onNodeWithText("Question 0 about the week ahead?").assertIsDisplayed()
    }

    @Test
    fun aQuestionWaitsAboveTheComposerAndAnsweringPutsItAway() {
        val model = ChatViewModel(repository, "tifa")
        show { ChatScreen(model, onBack = {}, onOpenProfile = {}) }
        compose.waitForIdle()
        compose.onNodeWithText("What's the plan for dinner? You were light on fibre at lunch.").assertIsDisplayed()
        snapshot("chat-ask")
        compose.onNodeWithText("Not sure yet").performClick()
        compose.waitForIdle()
        compose.onNodeWithText("What's the plan for dinner? You were light on fibre at lunch.").assertDoesNotExist()
        compose.onNodeWithText("Not sure yet").assertIsDisplayed()
    }

    @Test
    fun aMessageThatDidNotSendSaysSoAndCanBeSentAgainOrDeleted() {
        val retried = mutableListOf<String>()
        val deleted = mutableListOf<String>()
        val notSent =
            object : AgentRepository by repository {
                override fun conversation(agentId: String) =
                    flowOf(Conversation(listOf(ChatMessage("m1", Speaker.Owner, "Book the table for eight.", notSent = true))))

                override suspend fun retry(agentId: String, messageId: String) {
                    retried += messageId
                }

                override suspend fun discard(agentId: String, messageId: String) {
                    deleted += messageId
                }
            }
        val model = ChatViewModel(notSent, "tifa")
        show { ChatScreen(model, onBack = {}, onOpenProfile = {}) }
        compose.waitForIdle()
        compose.onNodeWithText("Didn't send · Tap to retry").assertIsDisplayed()
        snapshot("chat-not-sent")
        compose.onNodeWithText("Book the table for eight.").performClick()
        compose.waitForIdle()
        assertEquals(listOf("m1"), retried)
        compose.onNodeWithText("Delete").performClick()
        compose.waitForIdle()
        assertEquals(listOf("m1"), deleted)
    }

    @Test
    fun typingTurnsStopIntoSendAndAnIdleChatCannotSendNothing() {
        val model = ChatViewModel(repository, "tifa")
        show { ChatScreen(model, onBack = {}, onOpenProfile = {}) }
        compose.waitForIdle()
        compose.onNodeWithContentDescription("Send").assertIsNotEnabled()
        compose.onNodeWithContentDescription("Message Tifa").performTextInput("hello")
        compose.onNodeWithContentDescription("Send").assertIsEnabled()
        snapshot("chat-idle-typing")
    }

    @Test
    fun editingShowsNameWhatItIsForAndTheSwitches() {
        val model = AgentEditorViewModel(repository, EditorMode.Editing("tifa"))
        show(seconds = 2f) { AgentEditorScreen(model, onClose = {}, onFinished = {}) }
        compose.waitForIdle()
        compose.onNodeWithText("Tifa").assertIsDisplayed()
        compose.onNodeWithText("Tracks what you eat and keeps you training.").assertIsDisplayed()
        compose.onNodeWithText("What Tifa can do").assertIsDisplayed()
        compose.onNode(hasText("Calendar")).assertIsDisplayed()
        compose.onNode(hasContentDescription("Name")).assertExists()
        compose.onNodeWithText("Tifa found and added Health itself").assertIsDisplayed()
        snapshot("edit-tifa")
    }

    @Test
    fun whereRenoaCannotChangeAnAgentFromTheAppTheEditorSaysSoAndShowsNoEmptySwitches() {
        val notYet =
            object : AgentRepository by repository {
                override suspend fun profile(agentId: String): AgentProfile = repository.profile(agentId).copy(abilities = emptyList())

                override suspend fun edit(agentId: String, edit: EditProfile): AgentProfile = throw RepositoryException(Problem.NotYet, "not on the Host yet")
            }
        val model = AgentEditorViewModel(notYet, EditorMode.Editing("tifa"))
        show { AgentEditorScreen(model, onClose = {}, onFinished = {}) }
        compose.waitForIdle()
        compose.onAllNodesWithText("What Tifa can do").assertCountEquals(0)
        compose.onNodeWithContentDescription("Name").performTextReplacement("Tifa Lockhart")
        compose.onNodeWithText("Done").performClick()
        compose.waitForIdle()
        compose.onNodeWithText("That can't be changed from the app yet.").assertIsDisplayed()
    }

    @Test
    fun creatingNeedsANameBeforeBringingSomeoneToLife() {
        val model = AgentEditorViewModel(repository, EditorMode.Creating)
        show(seconds = 2f) { AgentEditorScreen(model, onClose = {}, onFinished = {}) }
        compose.waitForIdle()
        compose.onNodeWithText("Bring them to life").assertIsNotEnabled()
        compose.onNodeWithContentDescription("Name").performTextInput("Mira")
        compose.waitForIdle()
        compose.onNodeWithText("Bring Mira to life").assertIsEnabled()
        snapshot("create-mira")
    }

    @Test
    fun signInTakesACodeAndShowsWhyItStopped() {
        var submitted = 0
        var state: SignInState by mutableStateOf(SignInState.Ready)
        var code by mutableStateOf("")
        show { SignInScreen(state, code, { code = it }) { submitted++ } }
        compose.mainClock.advanceTimeBy(3_000)
        compose.waitForIdle()
        compose.onNodeWithText("Your agents, in your pocket.").assertIsDisplayed()
        compose.onNodeWithText("Continue with passkey").assertIsNotEnabled()
        compose.onNodeWithContentDescription("Sign-in code").performTextInput("ab12cd")
        compose.onNodeWithText("Continue with passkey").performClick()
        assertEquals(1, submitted)
        assertEquals("ab12cd", code)
        snapshot("sign-in")

        state = SignInState.Working
        compose.onNodeWithText("Waiting for your passkey…").assertIsNotEnabled()
        snapshot("sign-in-linking")
        state = SignInState.Failed("That code didn't work. A setup code works once, for 30 minutes.")
        compose.onNodeWithText("That code didn't work. A setup code works once, for 30 minutes.").assertIsDisplayed()
        snapshot("sign-in-failed")
    }

    @Test
    fun signInDrawsTheAgentAndItsAppsInPaper() {
        show(ThemeName.Paper, seconds = 2f) { SignInScreen(SignInState.Ready, "", {}) {} }
        // The tiles drop in one after another; let the last one land before the picture.
        compose.mainClock.advanceTimeBy(3_000)
        compose.waitForIdle()
        compose.onNodeWithContentDescription("An agent on the Renoa board, linked to Discord, Telegram and the web. This phone joins it once you sign in.").assertIsDisplayed()
        snapshot("sign-in-paper")
    }
}

/** Thursday 29 October 2026, 7:40 PM, when the demo evening is set. */
private val EVENING = java.time.ZonedDateTime.of(2026, 10, 29, 19, 40, 0, 0, java.time.ZoneId.of("Asia/Kolkata"))

/** A reply with each kind of markdown an agent is likely to send. */
private val MARKDOWN_REPLY =
    """
    ## Dinner plan

    Here is **what I found** for tonight:

    - Dal with `brown rice`
    - A side of salad

    ```kotlin
    val cups = mapOf("rice" to 1, "dal" to 2, "salt" to 0, "water" to 4, "ghee" to 1)
    ```

    ![A bowl of dal](https://example.com/dal.png)

    Recipe at [the blog](https://example.com/dal), [the shout](HTTP://example.com/up) or www.example.com.

    Not for here: [call them](tel:5551234), [run](javascript:alert(1)), [a file](content://media/1), [an app](intent://scan#Intent;end), [https://x.com](tel:1) or <me@example.com>.
    """.trimIndent()

/** A reply cut off mid-fence, so that streamed in it passes through every kind of unfinished markdown. */
private val UNFINISHED_REPLY =
    """
    Here is the plan. **Dal** with `brown rice` from [the blog](https://example.com/dal).

    ## Shopping

    1. Lentils
    - Basmati
      - Jasmine

    | Item | Cups |
    | --- | --- |
    | Quinoa | 1 |

    > Soak overnight.

    ```kotlin
    val shopping = listOf("dal", "rice"
    """.trimIndent()

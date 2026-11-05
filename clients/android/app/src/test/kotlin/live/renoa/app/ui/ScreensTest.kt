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
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasScrollToNodeAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.isSelectable
import androidx.compose.ui.test.isSelected
import androidx.compose.ui.test.isToggleable
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
import live.renoa.app.agents.OutgoingFile
import live.renoa.app.agents.Problem
import live.renoa.app.agents.RepositoryException
import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.performTextReplacement
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.badge.AgentHue
import live.renoa.app.badge.BadgeFrame.Companion.LIFT
import live.renoa.app.badge.HALF
import androidx.compose.ui.geometry.Offset
import kotlin.math.roundToInt
import live.renoa.app.agents.FakeAgentRepository
import live.renoa.app.badge.Badge
import live.renoa.app.badge.FACE_INK
import live.renoa.app.badge.PAPER_SKIN
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
import kotlin.test.assertFalse
import kotlin.test.assertNull
import live.renoa.app.agents.Feed
import live.renoa.app.agents.Note
import live.renoa.app.agents.NoteKind

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
    fun theMainAgentWearsPaperAndTheOthersTheirColourInGraphite() = facesIn(ThemeName.Graphite)

    @Test
    fun theMainAgentWearsPaperAndTheOthersTheirColourInPaper() = facesIn(ThemeName.Paper)

    /** Each face's eye is ink, and the skin beside it is Paper for the main agent and the agent's own colour for another. */
    private fun facesIn(theme: ThemeName) {
        val roster = listOf(AgentSummary("renoa", "Renoa", "", AgentState.Resting, primary = true), AgentSummary("tifa", "Tifa", "", AgentState.Resting))
        show(theme) {
            Column(Modifier.background(Renoa.colors.background).padding(20.dp), verticalArrangement = Arrangement.spacedBy(40.dp)) {
                roster.forEach { Badge(it, roster, 140.dp) }
            }
        }
        val picture = picture()
        val dp = picture.density / 160f
        // How many pixels one unit of the drawing spans, and where each tile's centre is drawn.
        val perUnit = 140 * dp / (HALF * 2)
        fun at(tile: Int, x: Float, y: Float): Int {
            val top = 20 * dp + tile * (140 + 40) * dp
            return picture.getPixel((20 * dp + 70 * dp + x * perUnit).roundToInt(), (top + 70 * dp + LIFT * 0.3f * perUnit + y * perUnit).roundToInt())
        }
        assertEquals(FACE_INK.toArgb(), at(0, -9f, -1f), "Renoa's eye")
        assertEquals(PAPER_SKIN.toArgb(), at(0, -26f, -6f), "Renoa's skin")
        assertEquals(FACE_INK.toArgb(), at(1, -9f, -1f), "Tifa's eye")
        assertEquals(theme.colors.partModel.toArgb(), at(1, -26f, -6f), "Tifa's skin, in clay")
        snapshot("badge-faces-${theme.name.lowercase()}")
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
        compose.onNodeWithContentDescription("Renoa, resting, your main agent").assertIsDisplayed()
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

    /** Opens the Renoa tab, on Routines. */
    private fun openPages() {
        compose.onNode(hasText("Renoa") and isSelectable()).performClick()
        compose.waitForIdle()
    }

    /** Scrolls Home's list to [matcher]. */
    private fun onList(matcher: SemanticsMatcher): SemanticsNodeInteraction {
        compose.onAllNodes(hasScrollToNodeAction()).onFirst().performScrollToNode(matcher)
        return compose.onNode(matcher)
    }

    @Test
    fun homeShowsTheDayRenoaOnTopAndWhoeverHasSomethingToSay() {
        home(seconds = 3.97f)
        compose.onNode(hasContentDescription("Today. Morning brief ran at 7:30 AM. GitHub check ran at 6:30 PM. Optimus: Three projects", substring = true)).assertIsDisplayed()
        compose.onNode(hasContentDescription("Prices and groceries failed at 7:15 PM. Tifa asks: What's the plan for dinner? You were light on fibre at lunch. Posts worth a reply at 8 PM. Evening check-in at 9 PM.", substring = true)).assertExists()
        compose.onNodeWithText("Next: Posts worth a reply at 8 PM \u00b7 2 ran \u00b7 1 didn\u2019t finish").assertIsDisplayed()
        compose.onNodeWithContentDescription("Renoa, working, your main agent").assertIsDisplayed()
        compose.onNodeWithText("Tifa wants your dinner plan, and Optimus found three projects worth a look tonight.").assertIsDisplayed()
        compose.onNodeWithText("What's the plan for dinner? You were light on fibre at lunch.").assertIsDisplayed()
        onList(hasText("Three projects are trending on GitHub that fit what you build. I can walk you through them tonight.")).assertIsDisplayed()
        onList(hasText("\u201cPrices and groceries\u201d didn\u2019t finish")).assertIsDisplayed()
        compose.onNodeWithContentDescription("Meet someone new").assertIsDisplayed()
        compose.onAllNodes(hasScrollToNodeAction()).onFirst().performScrollToIndex(0)
        snapshot("home")
    }

    @Test
    fun aQuestionIsAnsweredInPlaceAndAFailedRoutineRunsAgain() {
        home()
        compose.onNodeWithText("Dal and a salad").performClick()
        compose.waitForIdle()
        compose.onNodeWithText("Dal and a salad").assertDoesNotExist()
        assertNull(repository.agents.value.first { it.id == "tifa" }.glance.ask)

        onList(hasText("Run again")).performClick()
        compose.waitForIdle()
        compose.onNodeWithText("\u201cPrices and groceries\u201d didn\u2019t finish").assertDoesNotExist()
        assertTrue(repository.feed.value.notes.none { it.kind == NoteKind.Problem })
    }

    @Test
    fun tappingSomeoneOpensTheirChatAndPutsTheirNotesAway() {
        val opened = mutableListOf<String>()
        var made = 0
        home(onOpenChat = { opened += it }, onNewAgent = { made++ })
        compose.onNodeWithText("Tifa wants your dinner plan, and Optimus found three projects worth a look tonight.").performClick()
        compose.onNodeWithText("What's the plan for dinner? You were light on fibre at lunch.").performClick()
        onList(hasText("Three projects are trending on GitHub that fit what you build. I can walk you through them tonight.")).performClick()
        compose.waitForIdle()
        assertEquals(listOf("renoa", "tifa", "optimus"), opened)
        assertTrue(repository.feed.value.notes.none { it.id == "projects" }, "the note the owner opened is put away")
        onList(hasContentDescription("Echo, asleep")).performClick()
        assertEquals("echo", opened.last())

        compose.onNodeWithContentDescription("Meet someone new").performClick()
        assertEquals(1, made)
    }

    @Test
    fun theMainAgentsFailedRoutineRunsAgainFromItsCard() {
        val failed = Note("brief", "renoa", NoteKind.Problem, "\u201cMorning brief\u201d didn\u2019t finish", "Run it again.", EVENING.toInstant())
        val note = Note("gift", "renoa", NoteKind.Update, "Wants you to know", "The scarf is \u20b91,200 off tonight.", EVENING.toInstant())
        val repo = FakeAgentRepository(scope, now = { EVENING }, feed = Feed("1 thing needs you.", listOf(note, failed)))
        val model = HomeViewModel(repo)
        show { HomeScreen(model, onOpenChat = {}, onNewAgent = {}, onChangeColours = {}, now = { EVENING }) }
        compose.waitForIdle()
        compose.onNodeWithText("The scarf is \u20b91,200 off tonight.").assertIsDisplayed()
        compose.onNodeWithText("\u201cMorning brief\u201d didn\u2019t finish").assertIsDisplayed()
        compose.onAllNodesWithText("Run again").onFirst().performClick()
        compose.waitForIdle()
        assertTrue(repo.feed.value.notes.none { it.id == "brief" })
    }

    @Test
    fun aRoutineIsTurnedOnFromItsPageAndBackLeavesIt() {
        home()
        openPages()
        val shown = listOf("Morning brief", "Lunch photo", "GitHub check", "Prices and groceries", "Posts worth a reply", "Evening check-in")
        val order = shown.sortedBy { compose.onNode(isToggleable() and hasText(it)).fetchSemanticsNode().boundsInRoot.top }
        assertEquals(listOf("Posts worth a reply", "Evening check-in", "Morning brief", "GitHub check", "Prices and groceries", "Lunch photo"), order, "soonest first, the paused one last")
        compose.onNodeWithText("Tifa \u00b7 Every day at 1 PM").assertIsDisplayed()
        compose.onNodeWithText("Paused").assertIsDisplayed()
        compose.onNode(isToggleable() and hasText("Lunch photo")).performClick()
        compose.waitForIdle()
        assertTrue(repository.routines.value.first { it.id == "lunch" }.on)
        compose.onNodeWithText("Next: tomorrow at 1 PM").assertIsDisplayed()
        snapshot("home-routines")

        compose.activity.onBackPressedDispatcher.onBackPressed()
        compose.waitForIdle()
        compose.onNodeWithText("Next: tomorrow at 1 PM").assertDoesNotExist()
        compose.onNode(hasText("Agents") and isSelected()).assertIsDisplayed()
    }

    @Test
    fun theMemoryPageShowsWhatAgentsKeepAndTheAppsPageSwitchesThemForRenoa() {
        val opened = mutableListOf<String>()
        home(onOpenChat = { opened += it })
        openPages()
        compose.onNode(hasText("Memory") and isSelectable()).performClick()
        compose.waitForIdle()
        compose.onNodeWithText("Buys oat milk, never dairy.").assertIsDisplayed()
        compose.onNodeWithText("Renoa \u00b7 29 Oct \u00b7 Reminds you 30 Oct, 1 PM").assertIsDisplayed()
        snapshot("home-memory")
        compose.onNode(hasText("Renoa") and isSelectable()).performClick()
        compose.waitForIdle()
        compose.onNodeWithText("Buys oat milk, never dairy.").assertIsDisplayed()

        compose.onNode(hasText("Apps") and isSelectable()).performClick()
        compose.waitForIdle()
        compose.onNode(isToggleable() and hasText("GitHub")).performClick()
        compose.waitForIdle()
        assertFalse(kotlinx.coroutines.runBlocking { repository.apps("renoa") }.first { it.connection == "github" }.on)
        snapshot("home-apps")
        compose.onNodeWithText("Ask Renoa").performClick()
        assertEquals(listOf("renoa"), opened)
    }

    @Test
    fun theStripsLineOpensRoutinesAndTheAgentsTabComesBack() {
        home()
        compose.onNodeWithText("Next: Posts worth a reply at 8 PM \u00b7 2 ran \u00b7 1 didn\u2019t finish").performClick()
        compose.waitForIdle()
        compose.onNodeWithText("Tifa \u00b7 Every day at 1 PM").assertIsDisplayed()
        compose.onNode(hasText("Agents") and isSelectable()).performClick()
        compose.waitForIdle()
        compose.onNodeWithText("Tifa \u00b7 Every day at 1 PM").assertDoesNotExist()
        compose.onNodeWithText("What's the plan for dinner? You were light on fibre at lunch.").assertIsDisplayed()
    }

    @Test
    fun anEmptyRoutinesPageSaysHowToMakeOne() {
        val model = HomeViewModel(FakeAgentRepository(scope, now = { EVENING }, routineSeeds = emptyList()))
        show { HomeScreen(model, onOpenChat = {}, onNewAgent = {}, onChangeColours = {}, now = { EVENING }) }
        compose.waitForIdle()
        openPages()
        compose.onNodeWithText("No routines yet", substring = true).assertIsDisplayed()
    }

    @Test
    fun homeInPaper() {
        home(ThemeName.Paper, seconds = 3.97f)
        compose.onNodeWithText("What's the plan for dinner? You were light on fibre at lunch.").assertIsDisplayed()
        snapshot("home-paper")
        openPages()
        snapshot("home-routines-paper")
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
        compose.onNodeWithText("Make your first agent").assertDoesNotExist()
        compose.onNodeWithText("Nobody is here yet").assertDoesNotExist()
        compose.onNodeWithText("Routines").assertDoesNotExist()
        snapshot("home-connecting")

        // Once Renoa says no one is here, Home says so and offers to make someone.
        known.value = true
        compose.waitForIdle()
        compose.onNodeWithText("Make your first agent").assertIsDisplayed()
        compose.onNodeWithText(CHECKING_IN).assertDoesNotExist()
    }

    @Test
    fun theColourChangeLivesOnTheMark() {
        var changed = 0
        home(onChangeColours = { changed++ })
        compose.onNodeWithContentDescription("Change colours").performClick()
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

            override suspend fun send(agentId: String, text: String, files: List<OutgoingFile>) {
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
        val ground = ThemeName.Graphite.colors.background
        assertTrue(dal.spanStyles.any { it.item.background == ground && it.item.fontFamily == RenoaType.Code.fontFamily && dal.text.substring(it.start, it.end).trim() == "brown rice" }, "inline code")

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
    fun aLongNameLeavesTheBackButtonClear() {
        val long = "Tifa Lockhart, keeper of the seventh heaven and every meal plan"
        val renamed = MutableStateFlow(repository.agents.value.map { if (it.id == "tifa") it.copy(name = long) else it })
        val model = ChatViewModel(object : AgentRepository by repository { override val agents = renamed }, "tifa")
        var back = 0
        show { ChatScreen(model, onBack = { back++ }, onOpenProfile = {}) }
        compose.waitForIdle()
        compose.onNodeWithContentDescription("Back").performClick()
        assertEquals(1, back)
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

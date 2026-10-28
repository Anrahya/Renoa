package live.renoa.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.background
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasScrollToNodeAction
import androidx.compose.ui.test.hasText
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
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.FakeAgentRepository
import live.renoa.app.badge.Badge
import live.renoa.app.chat.ChatScreen
import live.renoa.app.chat.ChatViewModel
import live.renoa.app.editor.AgentEditorScreen
import live.renoa.app.editor.AgentEditorViewModel
import live.renoa.app.editor.EditorMode
import live.renoa.app.home.HomeScreen
import live.renoa.app.home.HomeViewModel
import live.renoa.app.signin.SignInScreen
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.ThemeName
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.onNodeWithTag
import live.renoa.app.chat.Reveal
import live.renoa.app.chat.RevealText
import live.renoa.app.chat.Revealed
import live.renoa.app.theme.RenoaType
import kotlin.test.assertTrue
import org.junit.After
import org.junit.Test
import kotlin.test.assertEquals

class ScreensTest : ScreenTest() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
    private val repository by lazy { FakeAgentRepository(scope) }

    @After
    fun tearDown() = scope.coroutineContext[kotlinx.coroutines.Job]!!.cancel()

    private fun badgesIn(theme: ThemeName) {
        show(theme, seconds = 3.97f) {
            Column(Modifier.background(Renoa.colors.background).padding(20.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    AgentState.entries.forEach { Badge("Tifa", it, 66.dp, label = "Tifa ${it.name}") }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    AgentState.entries.forEach { Badge("Renoa", it, 66.dp, primary = true, label = "Renoa ${it.name}") }
                }
            }
        }
        compose.onNodeWithContentDescription("Tifa Working, working").assertIsDisplayed()
        compose.onNodeWithContentDescription("Tifa Wants, wants you").assertIsDisplayed()
        snapshot("badges-${theme.name.lowercase()}")
    }

    @Test
    fun theFiveBadgeStatesSideBySideInGraphite() = badgesIn(ThemeName.Graphite)

    @Test
    fun theFiveBadgeStatesSideBySideInPaper() = badgesIn(ThemeName.Paper)

    @Test
    fun homeOpensWithTheLeadsNoteThenEveryoneAndWhatTheyNeed() {
        val model = HomeViewModel(repository)
        show(seconds = 3.97f) { HomeScreen(model, onOpenChat = {}, onNewAgent = {}, onChangeColours = {}) }
        compose.waitForIdle()
        compose.onNodeWithText("Evening, Yash. Tifa wants your dinner plan, and Optimus found three projects worth a look tonight.").assertIsDisplayed()
        compose.onNodeWithText("What's the plan for dinner? You were light on fibre at lunch.").assertIsDisplayed()
        compose.onNodeWithText("Dal and a salad").assertIsDisplayed()
        compose.onNodeWithText("With Kai'Sa · Picking three posts worth your reply…").assertIsDisplayed()
        assertEquals(2, compose.onAllNodesWithContentDescription("Renoa, working, your main agent").fetchSemanticsNodes().size, "her note and her row")
        snapshot("home")
    }

    @Test
    fun answeringFromHomePutsTheQuestionAway() {
        val model = HomeViewModel(repository)
        show { HomeScreen(model, onOpenChat = {}, onNewAgent = {}, onChangeColours = {}) }
        compose.waitForIdle()
        compose.onNodeWithText("Dal and a salad").performClick()
        compose.waitForIdle()
        compose.onNodeWithText("Dal and a salad").assertDoesNotExist()
        compose.onNodeWithText("Not sure yet").assertDoesNotExist()
    }

    @Test
    fun tappingSomeoneOpensTheirChatAndTheLastRowMakesSomeoneNew() {
        val opened = mutableListOf<String>()
        var made = 0
        val model = HomeViewModel(repository)
        show { HomeScreen(model, onOpenChat = { opened += it }, onNewAgent = { made++ }, onChangeColours = {}) }
        compose.waitForIdle()
        compose.onNodeWithText("Optimus").performClick()
        compose.onNodeWithText("Evening, Yash. Tifa wants your dinner plan, and Optimus found three projects worth a look tonight.").performClick()
        compose.onNode(hasScrollToNodeAction()).performScrollToNode(hasContentDescription("Meet someone new"))
        compose.onNodeWithContentDescription("Meet someone new").performClick()
        assertEquals(listOf("optimus", "renoa"), opened)
        assertEquals(1, made)
    }

    @Test
    fun homeInPaper() {
        val model = HomeViewModel(repository)
        show(ThemeName.Paper, seconds = 3.97f) { HomeScreen(model, onOpenChat = {}, onNewAgent = {}, onChangeColours = {}) }
        compose.waitForIdle()
        snapshot("home-paper")
    }

    @Test
    fun theColourChangeLivesOnTheMark() {
        var changed = 0
        val model = HomeViewModel(repository)
        show { HomeScreen(model, onOpenChat = {}, onNewAgent = {}, onChangeColours = { changed++ }) }
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

    @Test
    fun messagesAlreadyThereWhenAChatOpensShowWholeAtOnce() {
        val model = ChatViewModel(repository, "tifa")
        compose.mainClock.autoAdvance = false
        show { ChatScreen(model, onBack = {}, onOpenProfile = {}) }
        repeat(3) {
            compose.waitForIdle()
            compose.mainClock.advanceTimeByFrame()
        }
        val old = "Nice one. That's about 620 kcal and 22 g of protein, but light on fibre. Some fruit or a salad later would balance it."
        assertEquals(old.length, compose.onNodeWithText(old).fetchSemanticsNode().config[Revealed])
    }

    @Test
    fun aReplyGrowingEveryFrameShowsEachCharacterWithinTheLagLimit() {
        var text by mutableStateOf("")
        compose.mainClock.autoAdvance = false
        show { RevealText(text, complete = false, animate = true, style = RenoaType.Body, color = Color.White, modifier = Modifier.testTag("reply")) }
        // Three characters a frame is 180 a second, twice the base pace, so only the catch-up keeps it close.
        val lagFrames = (Reveal.MAX_LAG_SECONDS * 60).toInt() + 2
        val arrived = mutableListOf<Int>()
        repeat(90) { frame ->
            compose.runOnUiThread { text = "a".repeat(3 * (frame + 1)) }
            arrived += 3 * (frame + 1)
            compose.mainClock.advanceTimeByFrame()
            val shown = compose.onNodeWithTag("reply").fetchSemanticsNode().config[Revealed]
            if (frame >= lagFrames) assertTrue(shown >= arrived[frame - lagFrames], "frame $frame: $shown shown, ${arrived[frame - lagFrames]} due")
        }
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
    fun signInIsAPlaceholderThatGoesStraightIn() {
        var entered = 0
        show { SignInScreen(onContinue = { entered++ }) }
        compose.waitForIdle()
        compose.onNodeWithText("Your agents, in your pocket.").assertIsDisplayed()
        compose.onNodeWithText("Continue with passkey").performClick()
        assertEquals(1, entered)
        snapshot("sign-in")
    }
}

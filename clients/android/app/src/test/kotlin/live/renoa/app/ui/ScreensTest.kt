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
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.FakeAgentRepository
import live.renoa.app.badge.AgentLook
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
                    AgentState.entries.forEach { Badge(AgentLook.forName("Tifa"), it, 66.dp, label = "Tifa ${it.name}") }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    AgentState.entries.forEach { Badge(AgentLook.forName("Kai'Sa"), it, 66.dp, mini = true, label = "Kai ${it.name}") }
                }
            }
        }
        compose.onNodeWithContentDescription("Tifa Working, working").assertIsDisplayed()
        compose.onNodeWithContentDescription("Tifa Wants, wants you").assertIsDisplayed()
        snapshot("badges-${theme.name.lowercase()}")
    }

    @Test
    fun theFiveBadgeStatesSideBySideInMidnight() = badgesIn(ThemeName.Midnight)

    @Test
    fun theFiveBadgeStatesSideBySideInEmber() = badgesIn(ThemeName.Ember)

    @Test
    fun theFiveBadgeStatesSideBySideInCrimson() = badgesIn(ThemeName.Crimson)

    @Test
    fun homeShowsTheBusyStageAndTheHive() {
        val model = HomeViewModel(repository)
        show(seconds = 3.97f) { HomeScreen(model, onOpenChat = {}, onNewAgent = {}, onChangeColours = {}) }
        compose.waitForIdle()
        compose.onNodeWithText("Optimus").assertIsDisplayed()
        compose.onNodeWithText("Renoa").assertIsDisplayed()
        compose.onNodeWithContentDescription("Tifa, resting").assertIsDisplayed()
        compose.onNodeWithText("6 asleep · tidy up").assertIsDisplayed()
        snapshot("home")
    }

    @Test
    fun tidyingUpHidesTheSleepingAgentsAndOffersToBringThemBack() {
        val model = HomeViewModel(repository)
        show { HomeScreen(model, onOpenChat = {}, onNewAgent = {}, onChangeColours = {}) }
        compose.waitForIdle()
        compose.onNodeWithContentDescription("Nova, asleep").assertExists()
        compose.onNodeWithText("6 asleep · tidy up").performClick()
        compose.waitForIdle()
        compose.onNodeWithContentDescription("Nova, asleep").assertDoesNotExist()
        compose.onNodeWithText("Show 6 asleep").performClick()
        compose.waitForIdle()
        compose.onNodeWithContentDescription("Nova, asleep").assertExists()
    }

    @Test
    fun tappingAnAgentOpensItsChatAndTheNewSeatMakesAnAgent() {
        val opened = mutableListOf<String>()
        var made = 0
        val model = HomeViewModel(repository)
        show { HomeScreen(model, onOpenChat = { opened += it }, onNewAgent = { made++ }, onChangeColours = {}) }
        compose.waitForIdle()
        compose.onNodeWithContentDescription("Tifa, resting").performClick()
        compose.onNodeWithContentDescription("Make a new agent").performClick()
        assertEquals(listOf("tifa"), opened)
        assertEquals(1, made)
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
    fun chatShowsTheConversationTheWorkingStateAndStop() {
        val model = ChatViewModel(repository, "renoa")
        show(seconds = 3f) { ChatScreen(model, onBack = {}, onOpenProfile = {}) }
        compose.waitForIdle()
        compose.onNodeWithText("What's WebMCP, and should we support it? What would it cost to run?").assertIsDisplayed()
        compose.onNodeWithText("On it. I'll read up, and Kai'Sa can hunt down the costs.").assertIsDisplayed()
        compose.onNodeWithText("Connecting the dots").assertIsDisplayed()
        compose.onNodeWithContentDescription("Message Renoa").assertIsDisplayed()
        compose.onNodeWithContentDescription("Stop").assertIsEnabled()
        snapshot("chat-working")
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
        compose.onNodeWithText("Your training partner. Pushes hard, cheers louder.").assertIsDisplayed()
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

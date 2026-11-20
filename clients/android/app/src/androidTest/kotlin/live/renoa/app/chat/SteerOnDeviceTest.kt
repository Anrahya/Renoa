package live.renoa.app.chat

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.test.longClick
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.hasSetTextAction
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import live.renoa.app.agents.AgentRepository
import live.renoa.app.agents.FakeAgentRepository
import live.renoa.app.agents.OutgoingFile
import live.renoa.app.theme.RenoaTheme
import live.renoa.app.theme.ThemeName
import org.junit.After
import org.junit.Rule
import org.junit.Test
import kotlin.test.assertEquals

/** The composer on a phone, typed into and pressed through the touch screen, against the demo agents. */
class SteerOnDeviceTest {
    @get:Rule val compose = createComposeRule()

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)

    @After
    fun tearDown() = scope.cancel()

    @Test
    fun whileTheAgentWorksATapJoinsItsTurnAndHoldingSendsOnceItIsDone() {
        val steers = mutableListOf<Pair<String, Boolean>>()
        val demo = FakeAgentRepository(scope)
        val recording =
            object : AgentRepository by demo {
                override suspend fun send(agentId: String, text: String, files: List<OutgoingFile>, steer: Boolean) {
                    steers += text to steer
                }
            }
        val model = ChatViewModel(recording, "renoa")
        compose.setContent {
            RenoaTheme(ThemeName.Graphite, reducedMotion = true, clock = remember { mutableFloatStateOf(0f) }) {
                Box(Modifier.fillMaxSize()) { ChatScreen(model, onBack = {}, onOpenProfile = {}) }
            }
        }
        compose.onNodeWithContentDescription("Stop").assertIsDisplayed()
        compose.onNodeWithText(STEER_HINT).assertDoesNotExist()

        compose.onNode(hasSetTextAction()).performTextInput("Make it two posts")
        compose.onNodeWithText(STEER_HINT).assertIsDisplayed()
        compose.onNodeWithContentDescription("Send").performClick()
        compose.waitUntil { steers.size == 1 }

        compose.onNode(hasSetTextAction()).performTextInput("And one about phones")
        compose.onNodeWithContentDescription("Send").performTouchInput { longClick() }
        compose.onNodeWithText(SEND_WHEN_DONE).performClick()
        compose.waitUntil { steers.size == 2 }

        assertEquals(listOf("Make it two posts" to true, "And one about phones" to false), steers)
    }
}

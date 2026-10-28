package live.renoa.app

import android.provider.Settings
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.performTextReplacement
import androidx.compose.ui.test.junit4.StateRestorationTester
import androidx.lifecycle.ViewModelProvider
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import live.renoa.app.agents.FakeAgentRepository
import live.renoa.app.nav.EntryStores
import live.renoa.app.ui.ScreenTest
import org.junit.After
import org.junit.Test
import kotlin.test.assertEquals

/** The whole app on a real activity: the screens, the back stack, and what each screen keeps between visits. */
class RenoaAppTest : ScreenTest() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
    private val repository by lazy { FakeAgentRepository(scope) }

    @After
    fun tearDown() = scope.coroutineContext[Job]!!.cancel()

    private fun start() {
        // The phone's animation scale is off, so no badge loops and the screen can settle.
        Settings.Global.putFloat(compose.activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        compose.setContent { RenoaApp(repository) }
        compose.onNodeWithText("Continue with passkey").performClick()
        compose.waitForIdle()
    }

    private fun back() {
        compose.activity.onBackPressedDispatcher.onBackPressed()
        compose.waitForIdle()
    }

    private fun makeAnAgent(name: String) {
        compose.onNodeWithContentDescription("Make a new agent").performClick()
        compose.waitForIdle()
        compose.onNodeWithContentDescription("Name").performTextInput(name)
        compose.onNodeWithText("Bring $name to life").performClick()
        compose.waitForIdle()
        compose.onNodeWithContentDescription("Message $name").assertIsDisplayed()
    }

    @Test
    fun aSecondAgentCanBeMadeAfterTheFirstFromAnEmptyForm() {
        start()
        makeAnAgent("Mira")
        back()
        compose.onNodeWithContentDescription("Mira, resting").assertIsDisplayed()

        compose.onNodeWithContentDescription("Make a new agent").performClick()
        compose.waitForIdle()
        compose.onNodeWithText("Bring them to life").assertIsNotEnabled()
        compose.onNodeWithContentDescription("Name").performTextInput("Kira")
        compose.onNodeWithText("Bring Kira to life").performClick()
        compose.waitForIdle()

        compose.onNodeWithContentDescription("Message Kira").assertIsDisplayed()
        back()
        compose.onNodeWithContentDescription("Mira, resting").assertIsDisplayed()
        compose.onNodeWithContentDescription("Kira, resting").assertIsDisplayed()
    }

    @Test
    fun aProfileCanBeOpenedAndKeptMoreThanOnce() {
        start()
        compose.onNodeWithContentDescription("Tifa, resting").performClick()
        compose.waitForIdle()

        compose.onNodeWithText("Tifa").performClick()
        compose.waitForIdle()
        compose.onNodeWithContentDescription("Name").performTextReplacement("Tifa B")
        compose.onNodeWithText("Done").performClick()
        compose.waitForIdle()
        compose.onNodeWithContentDescription("Message Tifa B").assertIsDisplayed()

        compose.onNodeWithText("Tifa B").performClick()
        compose.waitForIdle()
        compose.onNodeWithText("Tifa B").assertIsDisplayed()
        compose.onNodeWithContentDescription("Name").performTextReplacement("Tifa C")
        compose.onNodeWithText("Done").performClick()
        compose.waitForIdle()
        compose.onNodeWithContentDescription("Message Tifa C").assertIsDisplayed()
    }

    @Test
    fun aRotationKeepsTheFormAndLeavingTheScreenDoesNot() {
        val restoration = StateRestorationTester(compose)
        Settings.Global.putFloat(compose.activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        restoration.setContent { RenoaApp(repository) }
        compose.onNodeWithText("Continue with passkey").performClick()
        compose.waitForIdle()
        compose.onNodeWithContentDescription("Make a new agent").performClick()
        compose.waitForIdle()
        compose.onNodeWithContentDescription("Name").performTextInput("Mira")

        restoration.emulateSavedInstanceStateRestore()
        compose.waitForIdle()
        compose.onNodeWithText("Bring Mira to life").assertIsEnabled()

        back()
        compose.onNodeWithContentDescription("Make a new agent").performClick()
        compose.waitForIdle()
        compose.onNodeWithText("Bring them to life").assertIsNotEnabled()
    }

    @Test
    fun aScreenThatHasLeftHoldsNothing() {
        start()
        val stores = ViewModelProvider(compose.activity)[EntryStores::class.java]
        assertEquals(1, stores.held, "only Home is on the stack; the sign-in screen is gone")

        makeAnAgent("Mira")
        assertEquals(2, stores.held, "Home and the new agent's chat; the create screen was replaced")

        back()
        assertEquals(1, stores.held)
    }
}

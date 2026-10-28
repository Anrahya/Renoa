package live.renoa.app

import android.provider.Settings
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsEnabled
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.SemanticsNodeInteraction
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasScrollToNodeAction
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollToNode
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
import live.renoa.app.signin.SignInCode
import live.renoa.rcp.SignInRefused
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
        compose.setContent { RenoaApp(repository, signedIn = true, signingIn = { _, _ -> }) }
        compose.waitForIdle()
    }

    private fun back() {
        compose.activity.onBackPressedDispatcher.onBackPressed()
        compose.waitForIdle()
    }

    /** Home is a list, so whoever is asked for may first need scrolling to. */
    private fun onHome(description: String): SemanticsNodeInteraction {
        compose.onNode(hasScrollToNodeAction()).performScrollToNode(hasContentDescription(description))
        return compose.onNodeWithContentDescription(description)
    }

    private fun meetSomeoneNew() {
        onHome("Meet someone new").performClick()
    }

    private fun makeAnAgent(name: String) {
        meetSomeoneNew()
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
        onHome("Mira, resting").assertIsDisplayed()

        meetSomeoneNew()
        compose.waitForIdle()
        compose.onNodeWithText("Bring them to life").assertIsNotEnabled()
        compose.onNodeWithContentDescription("Name").performTextInput("Kira")
        compose.onNodeWithText("Bring Kira to life").performClick()
        compose.waitForIdle()

        compose.onNodeWithContentDescription("Message Kira").assertIsDisplayed()
        back()
        onHome("Mira, resting").assertIsDisplayed()
        onHome("Kira, resting").assertIsDisplayed()
    }

    @Test
    fun aProfileCanBeOpenedAndKeptMoreThanOnce() {
        start()
        onHome("Tifa, wants you").performClick()
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
    fun signingInWithACodeOpensHomeAndAFailureSaysWhatToDo() {
        Settings.Global.putFloat(compose.activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        val codes = mutableListOf<SignInCode>()
        var refuse = true
        compose.setContent {
            RenoaApp(repository, signedIn = false, signingIn = { code, _ ->
                codes += code
                if (refuse) throw SignInRefused(401, "authentication_failed", "authentication failed")
            })
        }
        compose.onNodeWithText("Continue with passkey").assertIsNotEnabled()
        compose.onNodeWithContentDescription("Sign-in code").performTextInput("ab12cd")
        compose.onNodeWithText("Continue with passkey").performClick()
        compose.waitForIdle()
        compose.onNodeWithText("That code didn't work. A setup code works once, for 30 minutes.").assertIsDisplayed()

        refuse = false
        compose.onNodeWithText("Continue with passkey").performClick()
        compose.waitForIdle()
        assertEquals(listOf<SignInCode>(SignInCode.Setup("ab12cd"), SignInCode.Setup("ab12cd")), codes)
        onHome("Meet someone new").assertIsDisplayed()
    }

    @Test
    fun aRotationKeepsTheFormAndLeavingTheScreenDoesNot() {
        val restoration = StateRestorationTester(compose)
        Settings.Global.putFloat(compose.activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        restoration.setContent { RenoaApp(repository, signedIn = true, signingIn = { _, _ -> }) }
        compose.waitForIdle()
        meetSomeoneNew()
        compose.waitForIdle()
        compose.onNodeWithContentDescription("Name").performTextInput("Mira")

        restoration.emulateSavedInstanceStateRestore()
        compose.waitForIdle()
        compose.onNodeWithText("Bring Mira to life").assertIsEnabled()

        back()
        meetSomeoneNew()
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

package live.renoa.app.ui

import android.provider.Settings
import androidx.compose.foundation.layout.Column
import androidx.compose.material3.Text
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertIsNotEnabled
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.getUnclippedBoundsInRoot
import androidx.compose.ui.test.click
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.unit.dp
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.RenoaTheme
import live.renoa.app.theme.ThemeName
import live.renoa.design.PrimaryButton
import live.renoa.design.SecondaryButton
import live.renoa.design.WideButton
import org.junit.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

/** What every screen stands on: controls, the animation setting and the bundled licences. */
class FoundationsTest : ScreenTest() {
    @Test
    fun aSmallButtonTakesATapAcrossFortyEight() {
        var tapped = 0
        var next = 0
        show {
            Column {
                SecondaryButton("OK") { tapped++ }
                SecondaryButton("Next") { next++ }
            }
        }
        // Drawn about 44 tall, it keeps 48 to itself, so a tap just under its edge is still its own.
        val ok = compose.onNodeWithText("OK").getUnclippedBoundsInRoot()
        val below = compose.onNodeWithText("Next").getUnclippedBoundsInRoot()
        assertTrue(below.top - ok.top >= 48.dp, "the next button starts 48 below, not ${below.top - ok.top}")
        compose.onNodeWithText("OK").performTouchInput { click(Offset(centerX, height + 1.dp.toPx())) }
        assertEquals(1 to 0, tapped to next)
    }

    @Test
    fun buttonsTakeTapsWhenReadyAndShowASpinnerWithoutTakingTapsWhileBusy() {
        var tapped = 0
        var busyTaps = 0
        show {
            Column {
                PrimaryButton("Save") { tapped++ }
                WideButton("Continue", busy = true) { busyTaps++ }
            }
        }
        compose.onNodeWithText("Save").performClick()
        assertEquals(1, tapped)
        compose.onNodeWithContentDescription("Loading").assertIsDisplayed()
        compose.onNodeWithText("Continue").assertIsNotEnabled()
        assertEquals(0, busyTaps)
    }

    @Test
    fun turningAnimationsOffStopsTheMotionWithoutARestart() {
        val resolver = compose.activity.contentResolver
        Settings.Global.putFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f)
        compose.setContent {
            // A still clock, so the screen can settle while the setting under test says to move.
            val clock = remember { mutableFloatStateOf(0f) }
            RenoaTheme(ThemeName.Graphite, clock = clock) { Text(if (LocalReducedMotion.current) "still" else "moving") }
        }
        compose.onNodeWithText("moving").assertExists()
        Settings.Global.putFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        resolver.notifyChange(Settings.Global.getUriFor(Settings.Global.ANIMATOR_DURATION_SCALE), null)
        compose.waitForIdle()
        compose.onNodeWithText("still").assertExists()
    }

    @Test
    fun theFontAndIconLicencesShipInsideTheApp() {
        val shipped = compose.activity.assets.list("").orEmpty().toSet()
        for (licence in listOf("MIT-Phosphor.txt", "MIT-ThinkingOrbs.txt", "OFL-Inter.txt", "Apache-2.0-Codync.txt", "README.md")) {
            assertTrue(licence in shipped, "$licence is not in the APK's assets: $shipped")
        }
    }
}

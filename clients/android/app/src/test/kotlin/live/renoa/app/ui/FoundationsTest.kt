package live.renoa.app.ui

import android.provider.Settings
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Text
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.test.click
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.unit.dp
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.RenoaTheme
import live.renoa.app.theme.ThemeName
import org.junit.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertTrue

/** What every screen stands on: hit areas, the animation setting and the bundled licences. */
class FoundationsTest : ScreenTest() {
    @Test
    fun aSmallButtonTakesATapAcrossFortyEightAndGrowsWithAWrappedLabel() {
        val label = "Remind me tomorrow after I get back from the gym"
        var tapped = 0
        var next = 0
        show {
            Column {
                RenoaButton("OK", { tapped++ }, size = ButtonSize.Sm)
                RenoaButton(label, { next++ }, Modifier.width(120.dp), size = ButtonSize.Sm)
            }
        }
        // Drawn 32 tall, it keeps 48 to itself, so a tap just under its edge is still its own.
        compose.onNodeWithText("OK").performTouchInput { click(Offset(centerX, height + 6.dp.toPx())) }
        assertEquals(1 to 0, tapped to next)
        val layouts = mutableListOf<TextLayoutResult>()
        compose.onNodeWithText(label, useUnmergedTree = true).fetchSemanticsNode().config[SemanticsActions.GetTextLayoutResult].action!!(layouts)
        assertTrue(layouts.single().lineCount > 1, "the label wraps")
        assertFalse(layouts.single().didOverflowHeight, "and the button grows to show all of it")
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
        for (licence in listOf("MIT-Phosphor.txt", "OFL-IBMPlexMono.txt", "OFL-InstrumentSans.txt", "README.md")) {
            assertTrue(licence in shipped, "$licence is not in the APK's assets: $shipped")
        }
    }
}

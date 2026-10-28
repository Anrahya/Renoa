package live.renoa.app.ui

import android.graphics.Bitmap
import android.graphics.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.activity.ComponentActivity
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import live.renoa.app.theme.RenoaTheme
import live.renoa.app.theme.ThemeName
import org.junit.Rule
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import java.io.File

/**
 * Renders real screens under Robolectric. Each test also saves a picture to build/screens so
 * the screens can be looked at without a phone.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [36], qualifiers = "w390dp-h844dp-xxhdpi")
abstract class ScreenTest {
    @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()

    /** Shows [content] in a theme with the motion clock frozen at [seconds], so a picture is repeatable. */
    protected fun show(
        theme: ThemeName = ThemeName.Midnight,
        seconds: Float = 0f,
        reducedMotion: Boolean = false,
        content: @Composable () -> Unit,
    ) {
        compose.setContent {
            val frozen = remember { mutableFloatStateOf(seconds) }
            RenoaTheme(theme, reducedMotion, frozen) { Box(Modifier.fillMaxSize()) { content() } }
        }
    }

    /** Draws the whole window into a picture. The window is laid out at the size the test class asks for. */
    protected fun snapshot(name: String) {
        compose.waitForIdle()
        val root = compose.activity.window.decorView
        val bitmap = Bitmap.createBitmap(root.width, root.height, Bitmap.Config.ARGB_8888)
        root.draw(Canvas(bitmap))
        val dir = File("build/screens").apply { mkdirs() }
        File(dir, "$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
    }
}

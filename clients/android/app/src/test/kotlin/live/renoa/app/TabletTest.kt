package live.renoa.app

import android.provider.Settings
import androidx.compose.ui.test.onNodeWithContentDescription
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import live.renoa.app.agents.FakeAgentRepository
import live.renoa.app.ui.ScreenTest
import org.junit.After
import org.junit.Test
import org.robolectric.annotation.Config
import kotlin.test.assertTrue

/** A tablet, as wide as the owner's Xiaomi Pad held upright. */
@Config(sdk = [36], qualifiers = "w900dp-h1440dp-xhdpi")
class TabletTest : ScreenTest() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)

    @After
    fun tearDown() = scope.coroutineContext[Job]!!.cancel()

    @Test
    fun aTabletKeepsAPhonesColumnCentred() {
        Settings.Global.putFloat(compose.activity.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 0f)
        compose.setContent { RenoaApp({ FakeAgentRepository(scope) }, signedIn = true, signingIn = { _, _ -> }) }
        compose.waitForIdle()
        val density = compose.activity.resources.displayMetrics.density
        val mark = compose.onNodeWithContentDescription("Renoa, change colours").fetchSemanticsNode().boundsInRoot
        val margin = (900 - MAX_CONTENT_WIDTH.value) / 2
        assertTrue(mark.left / density >= margin, "Home starts ${mark.left / density}dp from the edge, inside the ${margin}dp margin")
        snapshot("tablet-home")
    }
}

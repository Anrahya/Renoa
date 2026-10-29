package live.renoa.app.signin

import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** The board fits whatever room the sign-in screen leaves it, in pixels at 3x, or is left out. */
class SignInBoardTest {
    private val density = 3f
    private val chip = Size(90f * density, 18f * density)
    private val gap = 6f * density
    private val maxScale = 22f * density / R
    private val minScale = 12f * density / R

    private fun fit(width: Float, height: Float) = fitBoard(width * density, height * density, chip, gap, maxScale, minScale)

    private fun assertInside(width: Float, height: Float) {
        val fit = fit(width, height) ?: return
        val box = Rect(0f, 0f, width * density, height * density)
        for (tile in TILES) {
            val at = fit.place(tile)
            // The drawn face sits 2.5 units inside the seat, and its side hangs 4 below it.
            val face = (R - 2.5f) * fit.scale
            val name = "${tile.seat}"
            assertTrue(at.x - face >= -0.5f && at.x + face <= box.right + 0.5f, "$width x $height: the tile at $name leaves the box sideways")
            assertTrue(at.y - face >= -0.5f && at.y + face + 4f * fit.scale <= box.bottom + 0.5f, "$width x $height: the tile at $name leaves the box")
            if (tile.label != null) {
                val label = fit.label(tile, chip, gap)
                assertTrue(label.left >= -0.5f && label.right <= box.right + 0.5f && label.bottom <= box.bottom + 0.5f, "$width x $height: the ${tile.label} label leaves the box at $label")
            }
        }
    }

    @Test
    fun everyTileAndLabelStaysInsideTheRoomItIsGiven() {
        for (width in listOf(270f, 320f, 390f, 600f, 900f)) {
            for (height in listOf(120f, 180f, 220f, 480f, 900f)) assertInside(width, height)
        }
    }

    @Test
    fun aPhoneShowsTheBoardAtTheLandingPagesTileSize() {
        val fit = fit(390f, 480f)
        assertEquals(maxScale, fit?.scale)
    }

    @Test
    fun aBoxTooSmallForReadableTilesLeavesTheBoardOut() {
        assertNull(fit(390f, 0f), "no room at all, as with the keyboard over a short phone")
        assertNull(fit(390f, 100f))
        assertNull(fit(150f, 480f))
    }

    @Test
    fun theDashedLineRunsOnlyWhileLinkingWithMotionOn() {
        var reads = 0
        val clock = { reads++; 2f }
        assertEquals(0f, linkPhase(linking = false, reduced = false, clock))
        assertEquals(0f, linkPhase(linking = true, reduced = true, clock))
        assertEquals(0, reads, "an idle board never reads the clock, so it never redraws")
        assertTrue(linkPhase(linking = true, reduced = false, clock) < 0f, "the dashes run towards the agent")
    }
}

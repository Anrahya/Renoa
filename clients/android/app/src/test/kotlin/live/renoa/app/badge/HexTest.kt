package live.renoa.app.badge

import kotlin.math.abs
import kotlin.math.hypot
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class HexTest {
    private fun near(a: Float, b: Float) = assertTrue(abs(a - b) < 1e-3f, "$a is not near $b")

    @Test
    fun aSharpHexHasItsVerticesOnTheCircumcircle() {
        val corners = hexCorners(10f, 20f, 36f, 0f)
        assertEquals(6, corners.size)
        corners.forEach {
            near(hypot(it.control.x - 10f, it.control.y - 20f), 36f)
            assertEquals(it.start, it.control, "no rounding means the edge runs straight into the vertex")
            assertEquals(it.end, it.control)
        }
        near(corners[0].control.x, 46f)
        near(corners[0].control.y, 20f)
    }

    @Test
    fun roundingCutsTheCornerAtTheRequestedDistanceAndNoFurtherThanAThirdOfTheRadius() {
        val r = 36f
        val rounded = hexCorners(0f, 0f, r, 4f)[0]
        // The circumradius equals the edge length, so cutting 4/36 of the edge is 4 units from the vertex.
        near(hypot(rounded.start.x - rounded.control.x, rounded.start.y - rounded.control.y), 4f)
        near(hypot(rounded.end.x - rounded.control.x, rounded.end.y - rounded.control.y), 4f)

        val capped = hexCorners(0f, 0f, r, 1000f)[0]
        near(hypot(capped.start.x - capped.control.x, capped.start.y - capped.control.y), 0.3f * r)
    }

    @Test
    fun neighbouringTilesAreOneTileApartCentreToCentre() {
        val r = 29.5f
        val a = Seat(0, 0).centre(r)
        for ((q, s) in listOf(1 to 0, 1 to -1, 0 to -1, -1 to 0, -1 to 1, 0 to 1)) {
            val b = Seat(q, s).centre(r)
            near(hypot(b.x - a.x, b.y - a.y), r * 1.7320508f)
        }
    }
}

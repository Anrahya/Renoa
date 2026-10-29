package live.renoa.app.badge

import androidx.compose.ui.graphics.Path
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.min
import kotlin.math.sin
import kotlin.math.sqrt

data class Pt(val x: Float, val y: Float)

/** One rounded corner of a hexagon: the straight edge ends at [start], bends round [control], and leaves at [end]. */
data class HexCorner(val start: Pt, val control: Pt, val end: Pt)

/**
 * The corners of a flat-top hexagon of circumradius [r] centred on ([cx], [cy]), each rounded by
 * up to [corner] along both edges. This is the one tile shape every part of the app is drawn in.
 */
fun hexCorners(cx: Float, cy: Float, r: Float, corner: Float): List<HexCorner> {
    val vertices = List(6) { Pt(cx + r * cos(PI / 3 * it).toFloat(), cy + r * sin(PI / 3 * it).toFloat()) }
    val t = min(corner, r * 0.3f) / r
    return List(6) { i ->
        val previous = vertices[(i + 5) % 6]
        val here = vertices[i]
        val next = vertices[(i + 1) % 6]
        HexCorner(
            start = Pt(here.x + (previous.x - here.x) * t, here.y + (previous.y - here.y) * t),
            control = here,
            end = Pt(here.x + (next.x - here.x) * t, here.y + (next.y - here.y) * t),
        )
    }
}

fun hexPath(cx: Float, cy: Float, r: Float, corner: Float): Path =
    Path().apply {
        hexCorners(cx, cy, r, corner).forEachIndexed { i, c ->
            if (i == 0) moveTo(c.start.x, c.start.y) else lineTo(c.start.x, c.start.y)
            quadraticTo(c.control.x, c.control.y, c.end.x, c.end.y)
        }
        close()
    }

/** Axial hex coordinates on a board of flat-top tiles: [q] runs along the row, [r] down the slanted column. */
data class Seat(val q: Int, val r: Int) {
    /** The centre of this seat when a tile has circumradius [radius]. */
    fun centre(radius: Float): Pt = Pt(radius * 1.5f * q, radius * sqrt(3f) * (r + q / 2f))
}

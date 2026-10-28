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

/** Axial hex coordinates: [q] runs along the row, [r] down the slanted column. */
data class Seat(val q: Int, val r: Int)

/** Seats spiral out from the centre, ring by ring, the way the sign-in tiles fill. */
object HexSpiral {
    private val directions = listOf(Seat(1, 0), Seat(1, -1), Seat(0, -1), Seat(-1, 0), Seat(-1, 1), Seat(0, 1))

    /** The seat for the [index]th tile: 0 is the centre, then 6 around it, then 12, and so on. */
    fun seat(index: Int): Seat {
        require(index >= 0) { "seat index must not be negative, got $index" }
        if (index == 0) return Seat(0, 0)
        var ring = 1
        var first = 1
        while (index >= first + 6 * ring) {
            first += 6 * ring
            ring++
        }
        var offset = index - first
        var seat = Seat(directions[0].q * ring, directions[0].r * ring)
        if (offset == 0) return seat
        // From the ring's first seat, walk `ring` steps in each of the directions 2, 3, 4, 5, 0, 1.
        for (leg in 0 until 6) {
            val d = directions[(leg + 2) % 6]
            repeat(ring) {
                seat = Seat(seat.q + d.q, seat.r + d.r)
                offset--
                if (offset == 0) return seat
            }
        }
        error("seat $index is outside ring $ring")
    }

    /** The centre of [seat] when a tile has circumradius [radius]. */
    fun centre(seat: Seat, radius: Float): Pt =
        Pt(radius * 1.5f * seat.q, radius * sqrt(3f) * (seat.r + seat.q / 2f))
}

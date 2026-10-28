package live.renoa.app.signin

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import live.renoa.app.badge.HexSpiral
import live.renoa.app.badge.Seat
import live.renoa.app.badge.hexPath
import live.renoa.app.theme.LocalMotionClock
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaColors
import live.renoa.app.theme.RenoaEasing
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.BrandMarks
import live.renoa.app.ui.Glyphs
import live.renoa.app.ui.drawRenoaMark
import kotlin.math.abs

/** How the landing page draws a tile: the agent's core and parts, the apps linked to it, and this phone. */
private sealed interface Face {
    data object Core : Face

    data class Part(val pigment: (RenoaColors) -> Color, val tint: (RenoaColors) -> Color) : Face

    /** An app linked to the agent, marked with [mark] on a [grid]-unit square. */
    data class App(val mark: String, val grid: Float, val label: String) : Face

    /** Renoa on the web, where the setup code comes from. */
    data object Web : Face

    /** This phone: not linked yet, so its tile and its line are dashed. */
    data object Phone : Face
}

private class Tile(val seat: Seat, val face: Face)

/**
 * One agent in the middle, with its six parts, and the apps around it on the board. Each app sits
 * three steps out along a line through the core, so its link is one straight trace.
 */
private val TILES =
    listOf(
        Tile(Seat(0, 0), Face.Core),
        Tile(Seat(0, -1), Face.Part({ it.partModel }, { it.partModelTint })),
        Tile(Seat(1, -1), Face.Part({ it.partLoop }, { it.partLoopTint })),
        Tile(Seat(1, 0), Face.Part({ it.partContext }, { it.partContextTint })),
        Tile(Seat(0, 1), Face.Part({ it.partTools }, { it.partToolsTint })),
        Tile(Seat(-1, 1), Face.Part({ it.partSkills }, { it.partSkillsTint })),
        Tile(Seat(-1, 0), Face.Part({ it.partProfile }, { it.partProfileTint })),
        Tile(Seat(-3, 0), Face.App(BrandMarks.DISCORD, 24f, "Discord")),
        Tile(Seat(3, -3), Face.App(BrandMarks.TELEGRAM, 24f, "Telegram")),
        Tile(Seat(-3, 3), Face.Web),
        Tile(Seat(3, 0), Face.Phone),
    )

/** The landing page's tile, in board units: radius, grout, rounded corner and the depth of its side. */
private const val R = 60f
private const val FACE_R = R - 2.5f
private const val CORNER = 3f
private const val DEPTH = 4f

/** The board's extent in units: four and a half tiles out each way, plus a tile, and the labels' room below. */
private const val HALF_WIDTH = 4.5f * R + R + 40f
private const val HALF_HEIGHT = 2.6f * R + R + 20f

/**
 * The sign-in picture, drawn as the landing page ends: an agent on the board with the apps it
 * talks through. While [linking], the dashed line to this phone runs towards the agent.
 */
@Composable
internal fun SignInBoard(linking: Boolean, modifier: Modifier = Modifier) {
    val colors = Renoa.colors
    val reduced = LocalReducedMotion.current
    val clock = LocalMotionClock.current
    val measurer = rememberTextMeasurer()
    val label = RenoaType.Eyebrow.copy(fontSize = 9.5.sp, lineHeight = 10.sp)
    val marks = remember { TILES.map { (it.face as? Face.App)?.let { app -> PathParser().parsePathString(app.mark).toPath() } } }
    val phone = remember { PathParser().parsePathString(Glyphs.PHONE).toPath() }
    // The core lands first, then its parts, then the apps around it.
    val drops = remember { List(TILES.size) { Animatable(if (reduced) 1f else 0f) } }
    LaunchedEffect(Unit) {
        if (reduced) return@LaunchedEffect
        drops.forEachIndexed { i, drop ->
            launch {
                delay(100L + i * 80L + if (i > 6) 160L else 0L)
                drop.animateTo(1f, tween(700, easing = RenoaEasing.Ease))
            }
        }
    }

    Canvas(
        modifier.semantics {
            contentDescription = "An agent on the Renoa board, linked to Discord, Telegram and the web. This phone joins it once you sign in."
        },
    ) {
        // A tile is drawn about 22dp across its radius, as on the landing page, unless the space is smaller.
        val scale = minOf(size.width / (2 * HALF_WIDTH), size.height / (2 * HALF_HEIGHT), 22.dp.toPx() / R)
        val origin = Offset(size.width / 2, size.height / 2 - 10f * scale)
        val hairline = 1.25.dp.toPx() / scale
        val labels = mutableListOf<Pair<Offset, () -> Unit>>()
        withTransform({ translate(origin.x, origin.y); scale(scale, scale, Offset.Zero) }) {
            sockets(colors, scale, hairline)
            // The links, under the tiles: each runs from its app to the core.
            TILES.forEachIndexed { i, tile ->
                if (tile.face !is Face.App && tile.face !is Face.Web && tile.face !is Face.Phone) return@forEachIndexed
                val at = HexSpiral.centre(tile.seat, R)
                val p = drops[i].value
                val dash =
                    if (tile.face is Face.Phone) {
                        val run = if (linking && !reduced) -clock.value * 40f else 0f
                        PathEffect.dashPathEffect(floatArrayOf(6f / scale * 1.5f, 8f / scale * 1.5f), run)
                    } else {
                        null
                    }
                drawLine(colors.foreground.copy(alpha = 0.42f * p), Offset(at.x, at.y), Offset.Zero, strokeWidth = hairline * 1.2f, pathEffect = dash)
            }
            TILES.forEachIndexed { i, tile ->
                val at = HexSpiral.centre(tile.seat, R)
                val p = drops[i].value
                val lift = (1f - p) * -30f
                withTransform({ translate(0f, lift) }) { drawTile(tile, Offset(at.x, at.y), p, colors, hairline, marks[i], phone, linking) }
                val name =
                    when (val face = tile.face) {
                        is Face.App -> face.label
                        Face.Web -> "Web"
                        Face.Phone -> "This phone"
                        else -> null
                    }
                if (name != null) {
                    val text = measurer.measure(name.uppercase(), label)
                    val under = Offset(origin.x + at.x * scale, origin.y + (at.y + R + DEPTH) * scale + 6.dp.toPx())
                    labels += under to { drawLabel(text, under, tile.face, colors, p) }
                }
            }
        }
        labels.forEach { it.second() }
    }
}

/** The faint empty sockets the board is made of. */
private fun DrawScope.sockets(colors: RenoaColors, scale: Float, hairline: Float) {
    val halfW = size.width / scale / 2 + R
    val halfH = size.height / scale / 2 + R
    for (q in -10..10) {
        for (r in -12..12) {
            val c = HexSpiral.centre(Seat(q, r), R)
            if (abs(c.x) < halfW && abs(c.y) < halfH) drawPath(hexPath(c.x, c.y, FACE_R, CORNER), colors.border.copy(alpha = 0.6f), style = Stroke(hairline * 0.8f))
        }
    }
}

private fun DrawScope.drawTile(tile: Tile, at: Offset, p: Float, colors: RenoaColors, hairline: Float, mark: Path?, phone: Path, linking: Boolean) {
    fun Color.shown() = copy(alpha = alpha * p)
    val face = hexPath(at.x, at.y, FACE_R, CORNER)
    val side = hexPath(at.x, at.y + DEPTH, FACE_R, CORNER)
    fun block(fill: Color, edge: Color, sideFill: Color?, edgeWidth: Float = hairline, dashed: Boolean = false) {
        if (sideFill != null) {
            drawPath(side, sideFill.shown())
            drawPath(side, edge.shown(), style = Stroke(hairline))
        }
        drawPath(face, fill.shown())
        val effect = if (dashed) PathEffect.dashPathEffect(floatArrayOf(10f, 8f)) else null
        drawPath(face, edge.shown(), style = Stroke(edgeWidth, pathEffect = effect))
    }
    when (val f = tile.face) {
        Face.Core -> block(colors.foreground, colors.foreground, Color.Black)
        is Face.Part -> block(f.tint(colors), f.pigment(colors), f.pigment(colors))
        is Face.App -> {
            block(colors.card, colors.foreground, colors.input)
            if (mark != null) icon(mark, f.grid, at, colors.foreground.shown())
        }
        Face.Web -> {
            block(colors.card, colors.foreground, colors.foreground, edgeWidth = hairline * 1.8f)
            drawRenoaMark(colors.foreground.shown(), at, R * 0.78f)
        }
        Face.Phone -> {
            block(colors.background, if (linking) colors.foreground else colors.input, null, dashed = true)
            icon(phone, 256f, at, (if (linking) colors.foreground else colors.mutedForeground).shown())
        }
    }
}

/** A mark [R]-ish across, centred in its tile. */
private fun DrawScope.icon(path: Path, grid: Float, at: Offset, tint: Color) {
    val across = R * 0.7f
    withTransform({ translate(at.x - across / 2, at.y - across / 2); scale(across / grid, across / grid, Offset.Zero) }) { drawPath(path, tint) }
}

/** The mono label under an app: a dot for a linked app, a hollow one and a dashed edge for this phone. */
private fun DrawScope.drawLabel(text: TextLayoutResult, under: Offset, face: Face, colors: RenoaColors, p: Float) {
    val padX = 6.dp.toPx()
    val padY = 4.dp.toPx()
    val dot = 5.dp.toPx()
    val gap = 5.dp.toPx()
    val width = padX + dot + gap + text.size.width + padX
    val height = text.size.height + padY * 2
    val topLeft = Offset(under.x - width / 2, under.y)
    val web = face == Face.Web
    val off = face == Face.Phone
    val fill = if (web) colors.foreground else colors.card
    val ink = if (web) colors.primaryForeground else if (off) colors.mutedForeground else colors.foreground
    val corner = CornerRadius(2.dp.toPx())
    drawRoundRect(fill.copy(alpha = p), topLeft, Size(width, height), corner)
    val edge = if (off) PathEffect.dashPathEffect(floatArrayOf(3.dp.toPx(), 3.dp.toPx())) else null
    drawRoundRect((if (web) colors.foreground else colors.input).copy(alpha = p), topLeft, Size(width, height), corner, style = Stroke(1.dp.toPx(), pathEffect = edge))
    val dotCentre = Offset(topLeft.x + padX + dot / 2, topLeft.y + height / 2)
    if (off) {
        drawCircle(ink.copy(alpha = p), dot / 2 - 0.5.dp.toPx(), dotCentre, style = Stroke(1.dp.toPx()))
    } else {
        drawCircle(ink.copy(alpha = p), dot / 2, dotCentre)
    }
    drawText(text, ink.copy(alpha = p), Offset(topLeft.x + padX + dot + gap, topLeft.y + padY))
}

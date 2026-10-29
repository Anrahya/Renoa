package live.renoa.app.signin

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.layout.Spacer
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
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
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
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
import kotlin.math.ceil
import kotlin.math.floor
import kotlin.math.sqrt

/** How the landing page draws a tile: the agent's core and parts, the apps linked to it, and this phone. */
internal sealed interface Face {
    data object Core : Face

    data class Part(val pigment: (RenoaColors) -> Color, val tint: (RenoaColors) -> Color) : Face

    /** An app linked to the agent, marked with a Simple Icons [mark] on its 24-unit square. */
    data class App(val mark: String, val label: String) : Face

    /** Renoa on the web, where the setup code comes from. */
    data object Web : Face

    /** This phone: not linked yet, so its tile and its line are dashed. */
    data object Phone : Face
}

internal class Tile(val seat: Seat, val face: Face) {
    /** The mono label under the tile, or null for the agent's own tiles. */
    val label: String?
        get() =
            when (face) {
                is Face.App -> face.label
                Face.Web -> "Web"
                Face.Phone -> "This phone"
                Face.Core, is Face.Part -> null
            }
}

/**
 * One agent in the middle, with its six parts, and the apps around it on the board. Each app sits
 * three steps out along a line through the core, so its link is one straight trace.
 */
internal val TILES =
    listOf(
        Tile(Seat(0, 0), Face.Core),
        Tile(Seat(0, -1), Face.Part({ it.partModel }, { it.partModelTint })),
        Tile(Seat(1, -1), Face.Part({ it.partLoop }, { it.partLoopTint })),
        Tile(Seat(1, 0), Face.Part({ it.partContext }, { it.partContextTint })),
        Tile(Seat(0, 1), Face.Part({ it.partTools }, { it.partToolsTint })),
        Tile(Seat(-1, 1), Face.Part({ it.partSkills }, { it.partSkillsTint })),
        Tile(Seat(-1, 0), Face.Part({ it.partProfile }, { it.partProfileTint })),
        Tile(Seat(-3, 0), Face.App(BrandMarks.DISCORD, "Discord")),
        Tile(Seat(3, -3), Face.App(BrandMarks.TELEGRAM, "Telegram")),
        Tile(Seat(-3, 3), Face.Web),
        Tile(Seat(3, 0), Face.Phone),
    )

/** The landing page's tile, in board units: radius, the face inside the grout, its rounded corner and the depth of its side. */
internal const val R = 60f
private const val FACE_R = R - 2.5f
private const val CORNER = 3f
private const val DEPTH = 4f

/** Where the board sits in its box: [scale] pixels per board unit, with the core at [origin]. */
internal data class BoardFit(val scale: Float, val origin: Offset) {
    /** Where a tile's centre lands in the box. */
    fun place(tile: Tile): Offset = tile.seat.centre(R).let { Offset(origin.x + it.x * scale, origin.y + it.y * scale) }

    /** The box a label of [chip] size takes under [tile], [gap] below its side. */
    fun label(tile: Tile, chip: Size, gap: Float): Rect {
        val at = place(tile)
        return Rect(Offset(at.x - chip.width / 2, at.y + (R + DEPTH) * scale + gap), chip)
    }
}

/**
 * Fits the board in a [width] by [height] box, with every tile and every label of [chip] size
 * inside it, at no more than [maxScale]. Returns null when that would draw a tile smaller than
 * [minScale] allows, as with the keyboard open on a short phone: then the board is left out.
 */
internal fun fitBoard(width: Float, height: Float, chip: Size, gap: Float, maxScale: Float, minScale: Float): BoardFit? {
    val centres = TILES.map { it.seat.centre(R) }
    val labelled = TILES.indices.filter { TILES[it].label != null }
    val top = centres.maxOf { -it.y + FACE_R }
    val labelTop = labelled.maxOf { centres[it].y + R + DEPTH }
    val reach = centres.maxOf { abs(it.x) + FACE_R }
    val labelReach = labelled.maxOf { abs(centres[it].x) }
    val scale =
        minOf(
            maxScale,
            (height - gap - chip.height) / (top + labelTop),
            width / 2 / reach,
            (width / 2 - chip.width / 2) / labelReach,
        )
    if (scale < minScale) return null
    val above = top * scale
    // The lowest tiles are the labelled apps, so their labels are the bottom of the board.
    val below = labelTop * scale + gap + chip.height
    return BoardFit(scale, Offset(width / 2, (height - above - below) / 2 + above))
}

/**
 * How far the dashed line to this phone has run towards the agent, in board units. It runs only
 * while [linking] with motion on, and reads the clock only then, so an idle board never redraws.
 */
internal fun linkPhase(linking: Boolean, reduced: Boolean, seconds: () -> Float): Float = if (linking && !reduced) -seconds() * 40f else 0f

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
    val marks = remember { TILES.map { (it.face as? Face.App)?.let { app -> PathParser().parsePathString(app.mark).toPath() } } }
    val phone = remember { PathParser().parsePathString(Glyphs.PHONE).toPath() }
    // The core lands first, then its parts, then the apps around it. Turning animations off lands them all at once.
    val drops = remember { List(TILES.size) { Animatable(if (reduced) 1f else 0f) } }
    LaunchedEffect(reduced) {
        if (reduced) {
            drops.forEach { it.snapTo(1f) }
            return@LaunchedEffect
        }
        drops.forEachIndexed { i, drop ->
            launch {
                delay(100L + i * 80L + if (i > 6) 160L else 0L)
                drop.animateTo(1f, tween(700, easing = RenoaEasing.Ease))
            }
        }
    }

    Spacer(
        modifier
            .clipToBounds()
            .semantics {
                contentDescription = "An agent on the Renoa board, linked to Discord, Telegram and the web. This phone joins it once you sign in."
            }.drawWithCache {
                // Labels keep their size when the phone's text is enlarged, so they never cover the tiles.
                val fixedText = Density(density, fontScale = 1f)
                val style = RenoaType.Eyebrow.copy(fontSize = 9.5.sp, lineHeight = 10.sp)
                val texts = TILES.map { tile -> tile.label?.let { measurer.measure(it.uppercase(), style, density = fixedText) } }
                val chip = Chip(this)
                val chipSize = Size(texts.maxOf { it?.size?.width ?: 0 } + chip.extraWidth, texts.maxOf { it?.size?.height ?: 0 } + chip.padY * 2)
                val gap = 6.dp.toPx()
                val fit = fitBoard(size.width, size.height, chipSize, gap, maxScale = 22.dp.toPx() / R, minScale = 12.dp.toPx() / R)
                if (fit == null) return@drawWithCache onDrawBehind {}
                val hairline = 1.25.dp.toPx() / fit.scale
                val sockets = sockets(fit, size)
                val faces = TILES.map { it.seat.centre(R).let { c -> hexPath(c.x, c.y, FACE_R, CORNER) } }
                val sides = TILES.map { it.seat.centre(R).let { c -> hexPath(c.x, c.y + DEPTH, FACE_R, CORNER) } }
                val tileDash = PathEffect.dashPathEffect(floatArrayOf(10f, 8f))
                val linkDash = floatArrayOf(9f / fit.scale, 12f / fit.scale)
                onDrawBehind {
                    withTransform({ translate(fit.origin.x, fit.origin.y); scale(fit.scale, fit.scale, Offset.Zero) }) {
                        drawPath(sockets, colors.border.copy(alpha = 0.6f), style = Stroke(hairline * 0.8f))
                        // The links, under the tiles: each runs from its app to the core.
                        TILES.forEachIndexed { i, tile ->
                            if (tile.label == null) return@forEachIndexed
                            val at = tile.seat.centre(R)
                            val dash = if (tile.face == Face.Phone) PathEffect.dashPathEffect(linkDash, linkPhase(linking, reduced) { clock.value }) else null
                            drawLine(colors.foreground.copy(alpha = 0.42f * drops[i].value), Offset(at.x, at.y), Offset.Zero, strokeWidth = hairline * 1.2f, pathEffect = dash)
                        }
                        TILES.forEachIndexed { i, tile ->
                            val p = drops[i].value
                            withTransform({ translate(0f, (1f - p) * -30f) }) {
                                drawTile(tile, faces[i], sides[i], p, colors, hairline, tileDash, marks[i], phone, linking)
                            }
                        }
                    }
                    TILES.forEachIndexed { i, tile ->
                        val text = texts[i] ?: return@forEachIndexed
                        val width = text.size.width + chip.extraWidth
                        drawChip(text, fit.label(tile, Size(width, chipSize.height), gap), tile.face, colors, drops[i].value, chip)
                    }
                }
            },
    )
}

/** Every empty socket the box shows, as one path: the board the tiles sit on. */
private fun sockets(fit: BoardFit, size: Size): Path {
    val left = -fit.origin.x / fit.scale - R
    val right = (size.width - fit.origin.x) / fit.scale + R
    val top = -fit.origin.y / fit.scale - R
    val bottom = (size.height - fit.origin.y) / fit.scale + R
    val row = sqrt(3f) * R
    return Path().apply {
        for (q in floor(left / (1.5f * R)).toInt()..ceil(right / (1.5f * R)).toInt()) {
            for (r in floor(top / row - q / 2f).toInt()..ceil(bottom / row - q / 2f).toInt()) {
                val c = Seat(q, r).centre(R)
                addPath(hexPath(c.x, c.y, FACE_R, CORNER))
            }
        }
    }
}

private fun DrawScope.drawTile(
    tile: Tile,
    face: Path,
    side: Path,
    p: Float,
    colors: RenoaColors,
    hairline: Float,
    dash: PathEffect,
    mark: Path?,
    phone: Path,
    linking: Boolean,
) {
    fun Color.shown() = copy(alpha = alpha * p)
    fun block(fill: Color, edge: Color, sideFill: Color?, edgeWidth: Float = hairline, dashed: Boolean = false) {
        if (sideFill != null) {
            drawPath(side, sideFill.shown())
            drawPath(side, edge.shown(), style = Stroke(hairline))
        }
        drawPath(face, fill.shown())
        drawPath(face, edge.shown(), style = Stroke(edgeWidth, pathEffect = if (dashed) dash else null))
    }
    val at = tile.seat.centre(R).let { Offset(it.x, it.y) }
    when (val f = tile.face) {
        Face.Core -> block(colors.foreground, colors.foreground, Color.Black)
        is Face.Part -> block(f.tint(colors), f.pigment(colors), f.pigment(colors))
        is Face.App -> {
            block(colors.card, colors.foreground, colors.input)
            if (mark != null) icon(mark, 24f, at, colors.foreground.shown())
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

/** A mark on a [grid]-unit square, drawn most of a tile across and centred in it. */
private fun DrawScope.icon(path: Path, grid: Float, at: Offset, tint: Color) {
    val across = R * 0.7f
    withTransform({ translate(at.x - across / 2, at.y - across / 2); scale(across / grid, across / grid, Offset.Zero) }) { drawPath(path, tint) }
}

/** A label's padding and dot, in pixels. */
private class Chip(density: Density) {
    val padX = with(density) { 6.dp.toPx() }
    val padY = with(density) { 4.dp.toPx() }
    val dot = with(density) { 5.dp.toPx() }
    val gap = with(density) { 5.dp.toPx() }
    val extraWidth = padX + dot + gap + padX
}

/** The mono label under an app: a dot for a linked app, a hollow one and a dashed edge for this phone. */
private fun DrawScope.drawChip(text: TextLayoutResult, box: Rect, face: Face, colors: RenoaColors, p: Float, chip: Chip) {
    val web = face == Face.Web
    val off = face == Face.Phone
    val fill = if (web) colors.foreground else colors.card
    val ink = if (web) colors.primaryForeground else if (off) colors.mutedForeground else colors.foreground
    val corner = CornerRadius(2.dp.toPx())
    drawRoundRect(fill.copy(alpha = p), box.topLeft, box.size, corner)
    val edge = if (off) PathEffect.dashPathEffect(floatArrayOf(3.dp.toPx(), 3.dp.toPx())) else null
    drawRoundRect((if (web) colors.foreground else colors.input).copy(alpha = p), box.topLeft, box.size, corner, style = Stroke(1.dp.toPx(), pathEffect = edge))
    val dotCentre = Offset(box.left + chip.padX + chip.dot / 2, box.center.y)
    if (off) {
        drawCircle(ink.copy(alpha = p), chip.dot / 2 - 0.5.dp.toPx(), dotCentre, style = Stroke(1.dp.toPx()))
    } else {
        drawCircle(ink.copy(alpha = p), chip.dot / 2, dotCentre)
    }
    drawText(text, ink.copy(alpha = p), Offset(box.left + chip.padX + chip.dot + chip.gap, box.top + chip.padY))
}

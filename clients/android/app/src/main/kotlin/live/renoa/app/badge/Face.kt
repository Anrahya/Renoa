package live.renoa.app.badge

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.vector.PathParser
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.Cheeks
import live.renoa.app.agents.Eyes
import live.renoa.app.agents.HairBack
import live.renoa.app.agents.HairColour
import live.renoa.app.agents.HairFront
import live.renoa.app.agents.Look
import live.renoa.app.agents.Skin
import live.renoa.app.agents.Tile
import live.renoa.app.agents.Wear
import live.renoa.app.theme.RenoaColors
import live.renoa.app.theme.ThemeName

/**
 * The parts' lines, drawn as the faces board draws them: in a 100-unit square centred on (50, 50),
 * the head a circle of radius 22 at (50, 54) over a little of the shoulders.
 */
internal class FaceShapes {
    val long = path("M27 52 Q27 28 50 28 Q73 28 73 52 L77 92 L23 92 Z")
    val twinTails = path("M29 40 Q13 54 18 86 Q25 72 31 58 Z M71 40 Q87 54 82 86 Q75 72 69 58 Z")
    val short = path("M28 52 Q28 30 50 30 Q72 30 72 52 Q65 41 50 41 Q35 41 28 52 Z")
    val fringe = path("M27.5 56 Q26 29 50 29 Q74 29 72.5 56 L70 47 Q60 47 58 42 Q53 48 47 44 Q41 49 34 45 Q31 50 27.5 56 Z")
    val sidePart = path("M27.5 56 Q25 28 53 29 Q75 31 72.5 52 Q68 40 57 38 Q45 47 27.5 56 Z")
    val princess = path("M27.5 58 Q26 29 50 29 Q74 29 72.5 58 L70 46 L30 46 Z M27 46 L33.5 46 L32.5 74 L26 72 Z M73 46 L66.5 46 L67.5 74 L74 72 Z")
    val tuft = path("M49 30.5 Q45.5 18 56 13.5 Q50.5 21.5 54.5 30.5 Z")
    val blushLines = path("M33.5 63 l2.2 -2.4 M36.8 63.6 l2.2 -2.4 M40.1 64.2 l2.2 -2.4 M57.7 64.2 l2.2 -2.4 M61 63.6 l2.2 -2.4 M64.3 63 l2.2 -2.4")
    val lashes = path("M39.6 53.4 l-2.2 -1.6 M60.4 53.4 l2.2 -1.6")
    val catEyes = path("M37.6 56.4 Q42 51.2 46.4 55.6 Q42 59.4 37.6 56.4 Z M62.4 56.4 Q58 51.2 53.6 55.6 Q58 59.4 62.4 56.4 Z")
    val smile = path("M46 63.5 Q50 66.8 54 63.5")
    val raised = path("M37.5 49 q4 -3 8 -0.5 M54.5 48.5 q4 -2.5 8 0.5")
    val worried = path("M37.5 51 l7 -2.5 M62.5 51 l-7 -2.5")
    val wavy = path("M45 65 q2.5 -2 5 0 q2.5 2 5 0")
    val closed = path("M38.5 56 q3.5 2.6 7 0 M54.5 56 q3.5 2.6 7 0")
    val flat = path("M47.5 64.5 h5")
    val bridge = path("M48.2 56 h3.6")
    val bow = path("M64 33 L73.5 28 L72.5 38.5 Z M64 33 L55.5 26.5 L56.5 37 Z")
    val beanie = path("M27 47 Q27 23 50 23 Q73 23 73 47 Z")
    val band = path("M27 55 Q26 26 50 26 Q74 26 73 55")
    val ears = path("M29 37 L31 17 L44 30 Z M71 37 L69 17 L56 30 Z")
    val innerEars = path("M32.5 31 L33.5 22.5 L39.5 28.5 Z M67.5 31 L66.5 22.5 L60.5 28.5 Z")
    val star = path("M0,-6.5 L1.9,-2.6 L6.2,-2 L3.1,1 L3.8,5.3 L0,3.3 L-3.8,5.3 L-3.1,1 L-6.2,-2 L-1.9,-2.6 Z")
    val bubble = path("M24,-38h14a4,4 0 0 1 4,4v8a4,4 0 0 1 -4,4h-6l-4,4v-4h-4a4,4 0 0 1 -4,-4v-8a4,4 0 0 1 4,-4z")

    private fun path(d: String): Path = PathParser().parsePathString(d).toPath()
}

/** The ink every face is drawn in: Paper's foreground in either theme. */
internal val FACE_INK = ThemeName.Paper.colors.foreground

/** The main agent's tile: Paper's background in either theme, so she looks the same on both. */
internal val PAPER = ThemeName.Paper.colors.background

private val BLUSH = Color(0xFFE9A493)
private val BLUSH_LINES = Color(0xFFD9826E)
private val WEAR = Color.White

/** Renoa's own look, worn by the main agent whatever the Host picked for it. Her tile is Paper, not this one's. */
private val RENOA =
    Look(HairFront.Fringe, HairBack.None, HairColour.Ink, Skin.Fair, Eyes.Round, Cheeks.Blush, Wear.None, Tile.Clay, tuft = false)

/** Before the Host has said how an agent looks: a short-haired head on a quiet tile. */
private val PLAIN =
    Look(HairFront.Short, HairBack.None, HairColour.Grey, Skin.Light, Eyes.Round, Cheeks.None, Wear.None, Tile.Clay, tuft = false)

internal fun Skin.color(): Color =
    when (this) {
        Skin.Fair -> Color(0xFFF6DCC6)
        Skin.Light -> Color(0xFFE8B892)
        Skin.Medium -> Color(0xFFC68B62)
        Skin.Deep -> Color(0xFF8D5A3C)
    }

internal fun HairColour.color(): Color =
    when (this) {
        HairColour.Ink -> Color(0xFF2B2420)
        HairColour.Brown -> Color(0xFF6B4228)
        HairColour.Auburn -> Color(0xFF9A4A2C)
        HairColour.Blonde -> Color(0xFFD9B26A)
        HairColour.Grey -> Color(0xFFB9B3AB)
    }

/** The tile's colour: the parts' colours the board already uses for clay, cobalt and ochre. */
internal fun Tile.color(colors: RenoaColors): Color =
    when (this) {
        Tile.Clay -> colors.partModel
        Tile.Cobalt -> colors.partSkills
        Tile.Ochre -> colors.partLoop
    }

private fun Color.darker(by: Float): Color = lerp(this, Color.Black, by)

/** What a tile is filled with: Paper for the main agent, its own colour for any other, a quiet grey before the Host has said. */
internal fun tileColor(look: Look?, primary: Boolean, colors: RenoaColors): Color =
    when {
        primary -> PAPER
        look == null -> colors.secondary
        else -> look.tile.color(colors)
    }

/**
 * The head and shoulders of an agent wearing [look] in [state], on a tile already filled with
 * [tile]. The [primary] agent wears Renoa's own look and her star; an agent with no look yet wears a
 * plain one. Content looks ahead and smiles with its own eyes; busy looks down at its work, asking
 * raises its brows and opens its mouth, stuck knits its brows over a wavering mouth, and asleep
 * closes its eyes. [scale] takes the board's units to the tile's; the caller clips to the tile.
 * [finest] is the thinnest line worth drawing, in the board's units, so a small face keeps its lines.
 * Hair and head carry a rim a shade darker than the tile, so no skin or hair melts into its tile.
 */
internal fun DrawScope.drawFace(shapes: FaceShapes, look: Look?, primary: Boolean, state: AgentState, tile: Color, ink: Color, scale: Float, finest: Float) {
    val worn = if (primary) RENOA else look ?: PLAIN
    val skin = worn.skin.color()
    val hair = worn.hairColour.color()
    val rim = Stroke(maxOf(1.2f, finest), join = StrokeJoin.Round)
    val rimColor = (if (primary) Color(0xFFD9D4C8) else tile).darker(0.35f)
    val line = { width: Float -> Stroke(maxOf(width, finest), cap = StrokeCap.Round, join = StrokeJoin.Round) }
    fun shape(path: Path, color: Color) {
        drawPath(path, color)
        drawPath(path, rimColor, style = rim)
    }
    withTransform({
        scale(scale, scale, Offset.Zero)
        translate(-50f, -50f)
    }) {
        when (worn.hairBack) {
            HairBack.Long -> shape(shapes.long, hair)
            HairBack.TwinTails -> shape(shapes.twinTails, hair)
            HairBack.None, HairBack.Bun -> Unit
        }
        drawOval(if (primary) Color(0xFFD9D4C8) else tile.darker(0.28f), Offset(19f, 78f), Size(62f, 46f))
        if (worn.hairBack == HairBack.Bun) {
            drawCircle(hair, 9f, Offset(50f, 27f))
            drawCircle(rimColor, 9f, Offset(50f, 27f), style = rim)
        }
        drawCircle(skin, 22f, Offset(50f, 54f))
        drawCircle(rimColor, 22f, Offset(50f, 54f), style = rim)
        when (worn.hairFront) {
            HairFront.Short -> shape(shapes.short, hair)
            HairFront.Fringe -> shape(shapes.fringe, hair)
            HairFront.SidePart -> shape(shapes.sidePart, hair)
            HairFront.Princess -> shape(shapes.princess, hair)
            HairFront.Curls -> {
                for ((x, y, r) in CURLS) drawCircle(rimColor, r, Offset(x, y), style = rim)
                for ((x, y, r) in CURLS) drawCircle(hair, r, Offset(x, y))
            }
        }
        if (worn.hairBack == HairBack.TwinTails) {
            for (x in listOf(28.5f, 71.5f)) {
                drawCircle(WEAR, 3.2f, Offset(x, 42f))
                drawCircle(ink, 3.2f, Offset(x, 42f), style = line(0.9f))
            }
        }
        if (worn.tuft) shape(shapes.tuft, hair)
        when (worn.cheeks) {
            Cheeks.Lines -> drawPath(shapes.blushLines, BLUSH_LINES, style = line(1.2f))
            Cheeks.Blush -> for (x in listOf(37f, 63f)) drawCircle(BLUSH, 3.3f, Offset(x, 62f))
            Cheeks.Freckles -> for ((x, y) in FRECKLES) drawCircle(skin.darker(0.38f), 1f, Offset(x, y))
            Cheeks.None -> Unit
        }
        // What it wears goes under the face's lines, so a beanie never hides a raised brow.
        if (worn.wear != Wear.Glasses) drawWear(shapes, worn.wear, ink, line)
        fun pair(rx: Float, ry: Float, cy: Float, dx: Float = 0f) {
            for (x in listOf(42f, 58f)) drawOval(ink, Offset(x + dx - rx, cy - ry), Size(rx * 2, ry * 2))
        }
        when (state) {
            AgentState.Resting -> {
                when (worn.eyes) {
                    Eyes.Round -> pair(2.8f, 2.8f, 56f)
                    Eyes.Oval -> pair(2.3f, 3.4f, 56f)
                    Eyes.Lashes -> {
                        pair(2.4f, 3.2f, 56f)
                        drawPath(shapes.lashes, ink, style = line(1.3f))
                    }
                    Eyes.Sparkle -> {
                        pair(3.3f, 4.4f, 55.5f)
                        for (x in listOf(43.2f, 59.2f)) drawCircle(Color.White, 1.4f, Offset(x, 53.8f))
                        for (x in listOf(41f, 57f)) drawCircle(Color.White, 0.7f, Offset(x, 57.6f))
                    }
                    Eyes.Cat -> drawPath(shapes.catEyes, ink)
                }
                drawPath(shapes.smile, ink, style = line(1.8f))
            }
            AgentState.Working -> {
                pair(2.4f, 2.6f, 58.5f, dx = -1.5f)
                drawCircle(ink, 1.6f, Offset(50f, 65f))
            }
            AgentState.Wants -> {
                drawPath(shapes.raised, ink, style = line(1.6f))
                pair(2.6f, 3.6f, 55.5f)
                drawOval(ink, Offset(47.8f, 61.7f), Size(4.4f, 5.6f))
            }
            AgentState.Stuck -> {
                drawPath(shapes.worried, ink, style = line(1.6f))
                pair(2.5f, 2.5f, 56.5f)
                drawPath(shapes.wavy, ink, style = line(1.6f))
            }
            AgentState.Asleep -> {
                drawPath(shapes.closed, ink, style = line(1.6f))
                drawPath(shapes.flat, ink, style = line(1.6f))
            }
        }
        if (worn.wear == Wear.Glasses) drawWear(shapes, worn.wear, ink, line)
        if (primary) {
            withTransform({
                translate(66f, 34f)
                rotate(12f, Offset.Zero)
            }) {
                drawPath(shapes.star, Color.White)
                drawPath(shapes.star, ink, style = line(1f))
            }
        }
    }
}

private fun DrawScope.drawWear(shapes: FaceShapes, wear: Wear, ink: Color, line: (Float) -> Stroke) {
    when (wear) {
        Wear.None -> Unit
        Wear.Glasses -> {
            for (x in listOf(42f, 58f)) drawCircle(ink, 6.2f, Offset(x, 56f), style = line(1.6f))
            drawPath(shapes.bridge, ink, style = line(1.6f))
        }
        Wear.Bow -> {
            drawPath(shapes.bow, WEAR)
            drawPath(shapes.bow, ink, style = line(0.9f))
            drawCircle(WEAR, 2.4f, Offset(64f, 33f))
            drawCircle(ink, 2.4f, Offset(64f, 33f), style = line(0.9f))
        }
        Wear.Clip ->
            withTransform({ rotate(-24f, Offset(67.5f, 39.2f)) }) {
                drawRoundRect(WEAR, Offset(62f, 37f), Size(11f, 4.4f), CORNER_CLIP)
                drawRoundRect(ink, Offset(62f, 37f), Size(11f, 4.4f), CORNER_CLIP, style = line(0.9f))
            }
        Wear.Beanie -> {
            drawPath(shapes.beanie, WEAR)
            drawRoundRect(WEAR.darker(0.22f), Offset(25.5f, 43f), Size(49f, 8f), CORNER_BAND)
            drawCircle(WEAR, 4.5f, Offset(50f, 21f))
        }
        Wear.Headphones -> {
            drawPath(shapes.band, ink, style = line(3f))
            for (x in listOf(22f, 70f)) {
                drawRoundRect(WEAR, Offset(x, 50f), Size(8f, 13f), CORNER_CUP)
                drawRoundRect(ink, Offset(x, 50f), Size(8f, 13f), CORNER_CUP, style = line(1.2f))
            }
        }
        Wear.CatEars -> {
            drawPath(shapes.ears, WEAR)
            drawPath(shapes.ears, ink, style = line(1f))
            drawPath(shapes.innerEars, BLUSH)
        }
    }
}

private val CORNER_CLIP = androidx.compose.ui.geometry.CornerRadius(2.2f)
private val CORNER_BAND = androidx.compose.ui.geometry.CornerRadius(4f)
private val CORNER_CUP = androidx.compose.ui.geometry.CornerRadius(3.5f)

private val CURLS = listOf(Triple(31f, 45f, 7f), Triple(36f, 36f, 8f), Triple(45f, 31f, 8f), Triple(55f, 31f, 8f), Triple(64f, 36f, 8f), Triple(69f, 45f, 7f))

private val FRECKLES = listOf(35.5f to 60.5f, 38.5f to 62.5f, 35f to 64f, 64.5f to 60.5f, 61.5f to 62.5f, 65f to 64f)

package live.renoa.app.badge

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.vector.PathParser
import live.renoa.app.agents.AgentState
import live.renoa.app.theme.ThemeName

/**
 * The one thing that makes an agent's face its own. It comes from the agent's name, so the same
 * name wears the same trait on every screen, and the editor shows it change as the name is typed.
 * The main agent wears none: she has her blush and her star.
 */
enum class Trait {
    Fringe,
    Glasses,
    Freckles,
    Bun,
    ;

    companion object {
        fun of(name: String): Trait = entries[Math.floorMod(name.trim().lowercase().hashCode(), entries.size)]
    }
}

/** The face's lines, in tile units round the tile's centre. */
internal class FaceShapes {
    val fringe = path("M-20,-34L20,-34L29,-20Q14,-24 7,-15Q0,-25 -8,-16Q-14,-24 -29,-20Z")
    val bun = hexPath(0f, -37f, 7f, 2f)
    val smile = path("M-7,10Q0,16 7,10")
    val wavy = path("M-8,15q4,-4 8,0q4,4 8,0")
    val flat = path("M-4,13h8")
    val closed = path("M-14,0q5,4 10,0M4,0q5,4 10,0")
    val raised = path("M-18,-15q8,-5 15,-1M3,-16q7,-4 15,1")
    val worried = path("M-18,-12l13,-4M18,-12l-13,-4")
    val glasses = path("M-1,-1h2")
    val bubble = path("M24,-38h14a4,4 0 0 1 4,4v8a4,4 0 0 1 -4,4h-6l-4,4v-4h-4a4,4 0 0 1 -4,-4v-8a4,4 0 0 1 4,-4z")

    private fun path(d: String): Path = PathParser().parsePathString(d).toPath()
}

/** The face's ink: Paper's foreground in either theme, since every face is light enough to carry it. */
internal val FACE_INK = ThemeName.Paper.colors.foreground

/** The main agent's skin: Paper's background in either theme, so she looks the same on both. */
internal val PAPER_SKIN = ThemeName.Paper.colors.background

private val BLUSH = Color(0xFFE9A493)

/** What a trait drawn in the agent's own colour wears: its skin, a shade darker. */
internal fun traitShade(skin: Color): Color = lerp(skin, Color.Black, 0.3f)

/** A bun sits behind the face, so it is drawn before it. */
internal fun DrawScope.drawBun(shapes: FaceShapes, skin: Color) = drawPath(shapes.bun, traitShade(skin))

/**
 * The eyes, brows and mouth of an agent in [state], with its [trait] and, for the [primary] agent,
 * her blush. Content looks ahead and smiles, busy looks down at its work, asking raises its brows
 * and opens its mouth, worried knits its brows over a wavering mouth, and asleep closes its eyes.
 */
internal fun DrawScope.drawFace(shapes: FaceShapes, face: Path, state: AgentState, trait: Trait?, primary: Boolean, skin: Color, ink: Color) {
    val line = Stroke(3f, cap = StrokeCap.Round, join = StrokeJoin.Round)
    if (primary) {
        drawCircle(BLUSH, 4.5f, Offset(-17f, 9f))
        drawCircle(BLUSH, 4.5f, Offset(17f, 9f))
    }
    when (trait) {
        Trait.Fringe -> clipPath(face) { drawPath(shapes.fringe, traitShade(skin)) }
        Trait.Freckles -> for (x in listOf(-1f, 1f)) for ((dx, dy) in FRECKLES) drawCircle(traitShade(skin), 1.6f, Offset(x * dx, dy))
        else -> Unit
    }
    val eyeX = if (trait == Trait.Glasses) 10f else 9f
    fun eyes(at: Offset, rx: Float, ry: Float) {
        for (x in listOf(-eyeX, eyeX)) drawOval(ink, Offset(x + at.x - rx, at.y - ry), Size(rx * 2, ry * 2))
    }
    when (state) {
        AgentState.Resting -> eyes(Offset(0f, -1f), 3.8f, 5f)
        AgentState.Working -> eyes(Offset(-3f, 3f), 3.4f, 4f)
        AgentState.Wants -> {
            drawPath(shapes.raised, ink, style = line)
            eyes(Offset(0f, -1f), 4f, 5.4f)
        }
        AgentState.Stuck -> {
            drawPath(shapes.worried, ink, style = line)
            eyes(Offset(0f, 0f), 3.4f, 4.4f)
        }
        AgentState.Asleep -> drawPath(shapes.closed, ink, style = line)
    }
    if (trait == Trait.Glasses) {
        for (x in listOf(-10f, 10f)) drawCircle(ink, 9f, Offset(x, -1f), style = line)
        drawPath(shapes.glasses, ink, style = line)
    }
    when (state) {
        AgentState.Resting -> drawPath(shapes.smile, ink, style = line)
        AgentState.Working -> drawCircle(ink, 2.6f, Offset(0f, 14f))
        AgentState.Wants -> drawOval(ink, Offset(-3.4f, 9.8f), Size(6.8f, 8.4f))
        AgentState.Stuck -> drawPath(shapes.wavy, ink, style = line)
        AgentState.Asleep -> drawPath(shapes.flat, ink, style = line)
    }
}

private val FRECKLES = listOf(19f to 8f, 15f to 11f, 21f to 12.5f)

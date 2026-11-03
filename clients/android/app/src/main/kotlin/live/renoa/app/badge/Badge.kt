package live.renoa.app.badge

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.badge.BadgeFrame.Companion.DEPTH
import live.renoa.app.badge.BadgeFrame.Companion.LIFT
import live.renoa.app.badge.BadgeFrame.Companion.RADIUS
import live.renoa.app.theme.LocalMotionClock
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaColors
import live.renoa.app.theme.RenoaEasing
import live.renoa.app.theme.RenoaType

/** Half the canvas in tile units: room for the lift, the ring leaving the tile, and the primary agent's halo. */
internal const val HALF = RADIUS * 1.42f
private const val CORNER = 3f

/** Phosphor Star, fill weight (MIT; see clients/android/licenses). */
private const val STAR =
    "M234.29,114.85l-45,38.83L203,211.75a16.4,16.4,0,0,1-24.5,17.82L128,198.49,77.47,229.57A16.4,16.4,0,0,1,53,211.75l13.76-58.07-45-38.83A16.46,16.46,0,0,1,31.08,86l59-4.76,22.76-55.08a16.36,16.36,0,0,1,30.27,0l22.75,55.08,59,4.76a16.46,16.46,0,0,1,9.37,28.86Z"

private class Shapes {
    val face = hexPath(0f, 0f, RADIUS, CORNER)
    val halo = hexPath(0f, 0f, RADIUS + 6f, CORNER + 1f)
    val star = PathParser().parsePathString(STAR).toPath()
    val features = FaceShapes()
}

/**
 * [agent] as a tile, wearing what it wears on every screen: its hue on [roster], and, if it is
 * the main agent, Renoa's blush and star.
 */
@Composable
fun Badge(agent: AgentSummary, roster: List<AgentSummary>, size: Dp, modifier: Modifier = Modifier, label: String = agent.name, arriveAfterMillis: Int? = null) =
    Badge(agent.name, agent.state, size, mainAgent(roster)?.id == agent.id, AgentHue.of(agent.id, roster), modifier, label, arriveAfterMillis)

/**
 * An agent as a tile on the board, drawn the way the landing page draws a core: a face, a short
 * body below it, and a shadow on the socket. The tile has a face: an agent other than the main one
 * wears its [hue] and the [Trait] its name gives it. The [primary] agent, the one the owner talks to
 * first, is Paper with a blush, a star, and a light that runs round its edge. State shows in the
 * face (see [drawFace]) and in motion (see [BadgeMotion]); an agent that wants the owner holds up
 * a question bubble. With [arriveAfterMillis] the tile drops into its socket once, the way tiles
 * assemble on the landing page.
 */
@Composable
fun Badge(
    name: String,
    state: AgentState,
    size: Dp,
    primary: Boolean,
    hue: AgentHue,
    modifier: Modifier = Modifier,
    label: String = name,
    arriveAfterMillis: Int? = null,
) {
    val colors = Renoa.colors
    val reduced = LocalReducedMotion.current
    val clock = LocalMotionClock.current
    val shapes = remember { Shapes() }
    val measurer = rememberTextMeasurer()
    val question = remember(measurer) { measurer.measure("?", QUESTION_STYLE, density = UNIT_DENSITY) }
    val trait = if (primary) null else Trait.of(name)
    val arrival = remember { Animatable(if (arriveAfterMillis != null && !reduced) 0f else 1f) }
    LaunchedEffect(Unit) {
        if (arrival.value < 1f) {
            delay(arriveAfterMillis?.toLong() ?: 0L)
            arrival.animateTo(1f, tween(650, easing = RenoaEasing.Ease))
        }
    }
    val moving = BadgeMotion.animates(state, reduced) || (primary && !reduced)
    val words = state.describe() + if (primary) ", your main agent" else ""

    Canvas(
        modifier
            .size(size)
            .graphicsLayer { alpha = BadgeMotion.frame(state, 0f, reduced).alpha * arrival.value }
            .semantics { if (label.isNotEmpty()) contentDescription = "$label, $words" },
    ) {
        val t = if (moving) clock.value else 0f
        val frame = BadgeMotion.frame(state, t, reduced)
        val scale = this.size.minDimension / (HALF * 2)
        // Arriving, a tile falls the last 1.6 lifts into its socket, as on the landing page.
        val drop = (1f - arrival.value) * 1.6f
        withTransform({ translate(this.size.width / 2, this.size.height / 2 + LIFT * 0.3f * scale); scale(scale, scale, Offset.Zero) }) {
            drawTile(frame, frame.lift + drop, shapes, question, state, trait, primary, colors, hue.face(colors), if (primary) primarySweep(clock.value, reduced) ?: 0.125f else null)
        }
    }
}

/** [pigment] is the agent's hue, or null for a Paper face with an ink edge. */
private fun DrawScope.drawTile(
    frame: BadgeFrame,
    lift: Float,
    shapes: Shapes,
    question: TextLayoutResult,
    state: AgentState,
    trait: Trait?,
    primary: Boolean,
    colors: RenoaColors,
    pigment: Color?,
    sweep: Float?,
) {
    val up = -lift * LIFT
    val skin = if (frame.retiring) colors.muted else pigment ?: PAPER_SKIN
    val edge = pigment ?: FACE_INK
    if (!frame.retiring) {
        // The shadow stays on the socket and softens as the tile rises.
        withTransform({ translate(0f, DEPTH + 3f) }) { drawPath(shapes.face, Color.Black.copy(alpha = 0.32f / (1f + lift))) }
    }
    frame.ring?.let { p ->
        val r = 1f + 0.38f * p
        withTransform({ translate(0f, up); scale(r, r, Offset.Zero) }) {
            drawPath(shapes.face, edge.copy(alpha = 0.55f * (1f - p)), style = Stroke(1.6f / r))
        }
    }
    withTransform({ translate(0f, up) }) {
        if (sweep != null) drawHalo(shapes.halo, colors.foreground, sweep)
        if (trait == Trait.Bun) drawBun(shapes.features, skin)
        if (!frame.retiring) withTransform({ translate(0f, DEPTH) }) { drawPath(shapes.face, Color.Black) }
        drawPath(shapes.face, skin)
        drawFace(shapes.features, shapes.face, state, trait, primary, skin, if (frame.retiring) colors.mutedForeground else FACE_INK)
        when {
            frame.retiring -> drawPath(shapes.face, colors.border, style = Stroke(1.25f))
            frame.failed -> drawPath(shapes.face, colors.stateFailed, style = Stroke(2.5f))
            else -> drawPath(shapes.face, edge, style = Stroke(if (pigment == null) 2.5f else 1.25f))
        }
        if (sweep != null) {
            val s = 14f / 256f
            withTransform({ translate(-RADIUS * 0.92f, -RADIUS * 0.98f); scale(s, s, Offset.Zero) }) {
                drawPath(shapes.star, colors.background, style = Stroke(48f))
                drawPath(shapes.star, colors.foreground)
            }
        }
        frame.bubble?.let { scale ->
            withTransform({ scale(scale, scale, BUBBLE_CENTRE) }) {
                drawPath(shapes.features.bubble, colors.background, style = Stroke(4.4f, join = StrokeJoin.Round))
                drawPath(shapes.features.bubble, colors.foreground)
                drawText(question, color = colors.background, topLeft = BUBBLE_CENTRE - Offset(question.size.width / 2f, question.size.height / 2f + 2f))
            }
        }
    }
}

private val BUBBLE_CENTRE = Offset(33f, -36f)

/** The primary agent's halo: a faint outline round the tile with a light running along it, its head at [sweep] of a turn. */
private fun DrawScope.drawHalo(halo: Path, ink: Color, sweep: Float) {
    drawPath(halo, ink.copy(alpha = 0.18f), style = Stroke(1.2f))
    val stops = Array(HALO_STOPS + 1) { i -> val x = i / HALO_STOPS.toFloat(); x to ink.copy(alpha = haloLight(x, sweep)) }
    drawPath(halo, Brush.sweepGradient(*stops, center = Offset.Zero), style = Stroke(2.2f))
}

private const val HALO_STOPS = 48

/** How bright the halo is at [x] of a turn when the light's head is at [head]: full at the head, fading over a sixth of a turn behind it. */
internal fun haloLight(x: Float, head: Float): Float {
    val behind = ((head - x) % 1f + 1f) % 1f
    return if (behind < 1f / 6) 1f - behind * 6 else 0f
}

internal fun AgentState.describe(): String =
    when (this) {
        AgentState.Resting -> "resting"
        AgentState.Working -> "working"
        AgentState.Wants -> "wants you"
        AgentState.Stuck -> "stuck"
        AgentState.Asleep -> "asleep"
    }

private val UNIT_DENSITY = Density(1f, 1f)

private val QUESTION_STYLE = RenoaType.Label.copy(fontSize = 12.sp, lineHeight = 12.sp)

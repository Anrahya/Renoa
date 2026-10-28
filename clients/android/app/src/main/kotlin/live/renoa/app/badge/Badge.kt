package live.renoa.app.badge

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.sp
import live.renoa.app.agents.AgentState
import live.renoa.app.theme.LocalMotionClock
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.Renoa
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.sin
import kotlin.math.sqrt

private const val CORE_RADIUS = 36f
private const val ORB_RADIUS = 13f
private val ORB_DEGREES = listOf(-90, -30, 30, 90, 150, 210)

/** Where the six tiles sit: just clear of the core's edge, one tile-height out. */
private val ORB_DISTANCE = CORE_RADIUS * sqrt(3f) / 2 + 4 + ORB_RADIUS * sqrt(3f) / 2

private class Shapes(mini: Boolean) {
    val core = hexPath(0f, 0f, CORE_RADIUS, 4f)
    val coreBody = hexPath(0f, 3f, CORE_RADIUS, 4f)
    val orbCentres = ORB_DEGREES.map { Offset(cos(it * PI / 180).toFloat() * ORB_DISTANCE, sin(it * PI / 180).toFloat() * ORB_DISTANCE) }
    val orbs = orbCentres.map { hexPath(it.x, it.y, ORB_RADIUS, 2.5f) }
    val orbBodies = orbCentres.map { hexPath(it.x, it.y + 2.5f, ORB_RADIUS, 2.5f) }
    val glowRadius = if (mini) 40f else 66f
    val dotCentre = if (mini) Offset(26f, -26f) else Offset(50f, -46f)
    val dotRadius = if (mini) 6f else 7f
}

/**
 * An agent: its picture in a hex core, ringed by six tiles in its colours. State is motion:
 * resting breathes, working blinks around, wants-you rises with one dot, stuck greys with a red
 * dot, asleep fades. A [mini] badge drops the tiles and says working with one pulsing dot.
 */
@Composable
fun Badge(
    look: AgentLook,
    state: AgentState,
    size: Dp,
    modifier: Modifier = Modifier,
    mini: Boolean = false,
    label: String = "",
) {
    val colors = Renoa.colors
    val reduced = LocalReducedMotion.current
    val clock = LocalMotionClock.current
    val shapes = remember(mini) { Shapes(mini) }
    val measurer = rememberTextMeasurer()
    val letter = remember(look.letter, measurer) { measurer.measure(look.letter.toString(), LETTER_STYLE, density = UNIT_DENSITY) }
    val animates = BadgeMotion.animates(state, reduced)
    val fade = BadgeMotion.frame(state, mini, 0f, reduced).alpha
    val words = state.describe()

    Canvas(
        modifier
            .size(size)
            .graphicsLayer { alpha = fade }
            .semantics { contentDescription = if (label.isEmpty()) words else "$label, $words" },
    ) {
        val frame = BadgeMotion.frame(state, mini, if (animates) clock.value else 0f, reduced)
        val half = if (mini) 42f else 80f
        val scale = this.size.minDimension / (half * 2)
        withTransform({ translate(this.size.width / 2, this.size.height / 2); scale(scale, scale, Offset.Zero) }) {
            drawBadge(frame, look, shapes, letter, colors.background, colors.foreground, colors.stateFailed, colors.stateRunning)
        }
    }
}

private fun DrawScope.drawBadge(
    frame: BadgeFrame,
    look: AgentLook,
    shapes: Shapes,
    letter: TextLayoutResult,
    background: Color,
    foreground: Color,
    failed: Color,
    running: Color,
) {
    fun tint(c: Color) = if (frame.grayscale) c.grayscale(frame.brightness) else c

    if (frame.glowAlpha > 0f) {
        val glow = tint(look.main).copy(alpha = frame.glowAlpha)
        drawCircle(Brush.radialGradient(listOf(glow, glow.copy(alpha = 0f)), Offset.Zero, shapes.glowRadius), shapes.glowRadius, Offset.Zero)
    }
    if (frame.shadowAlpha > 0f) {
        withTransform({ translate(0f, 18f); scale(1f, 0.3f, Offset.Zero) }) {
            drawCircle(Brush.radialGradient(listOf(Color.Black.copy(alpha = frame.shadowAlpha), Color.Transparent), Offset.Zero, 56f), 56f, Offset.Zero)
        }
    }
    withTransform({ translate(0f, frame.lift) }) {
        frame.orbs.forEachIndexed { i, orb ->
            val centre = shapes.orbCentres[i]
            withTransform({ scale(orb.scale, orb.scale, centre) }) {
                drawPath(shapes.orbBodies[i], Color.Black.copy(alpha = 0.35f * orb.alpha))
                drawPath(shapes.orbs[i], tint(look.tones[i]).copy(alpha = orb.alpha))
                drawPath(shapes.orbs[i], Color.White.copy(alpha = 0.14f * orb.alpha), style = Stroke(1f))
            }
        }
        withTransform({ scale(frame.coreScale, frame.coreScale, Offset.Zero) }) {
            drawPath(shapes.coreBody, Color.Black.copy(alpha = 0.4f))
            clipPath(shapes.core) {
                drawRect(tint(look.ground), Offset(-50f, -50f), Size(100f, 100f))
                drawCircle(tint(look.bloom).copy(alpha = 0.7f), 34f, Offset(-12f, -16f))
                drawCircle(tint(look.spot).copy(alpha = 0.5f), 22f, Offset(20f, 22f))
                drawText(letter, topLeft = Offset(-letter.size.width / 2f, 12f - letter.firstBaseline))
            }
            drawPath(shapes.core, Color.White.copy(alpha = 0.18f), style = Stroke(1.2f))
        }
        frame.dot?.let { dot ->
            val fill = when (dot) {
                Dot.Calling -> foreground
                Dot.Failed -> failed
                Dot.Running -> running
            }
            val r = shapes.dotRadius * frame.dotScale
            drawCircle(fill, r, shapes.dotCentre)
            drawCircle(background, r, shapes.dotCentre, style = Stroke(3f))
        }
    }
}

private fun Color.grayscale(brightness: Float): Color {
    val l = (0.299f * red + 0.587f * green + 0.114f * blue) * brightness
    return Color(l, l, l, alpha)
}

/** TalkBack words; they are never drawn. */
internal fun AgentState.describe(): String =
    when (this) {
        AgentState.Resting -> "resting"
        AgentState.Working -> "working"
        AgentState.Wants -> "wants you"
        AgentState.Stuck -> "stuck"
        AgentState.Asleep -> "asleep"
    }

private val UNIT_DENSITY = Density(1f, 1f)
private val LETTER_STYLE = TextStyle(fontSize = 34.sp, fontWeight = FontWeight.SemiBold, color = Color.White.copy(alpha = 0.92f))

package live.renoa.app.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import live.renoa.app.badge.hexPath
import live.renoa.app.theme.LocalMotionClock
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.RenoaEasing
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.sin

/** Parses SVG path data once per distinct string. */
@Composable
fun rememberSvgPath(data: String): Path = remember(data) { PathParser().parsePathString(data).toPath() }

/** A line icon from SVG path data on a 24 by 24 grid, the way the design draws them. */
@Composable
fun PathIcon(data: String, tint: Color, modifier: Modifier = Modifier, size: Dp = 22.dp, strokeWidth: Float = 1.7f) {
    val path = rememberSvgPath(data)
    Canvas(modifier.size(size)) {
        val s = this.size.minDimension / 24f
        withTransform({ scale(s, s, Offset.Zero) }) {
            drawPath(path, tint, style = Stroke(strokeWidth, cap = StrokeCap.Round, join = StrokeJoin.Round))
        }
    }
}

object Glyphs {
    const val BACK = "M15 5l-7 7 7 7"
    const val CLOSE = "M6 6l12 12M18 6L6 18"
    const val SEND = "M12 19V5M6 11l6-6 6 6"
    const val MOON = "M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z"
    const val PASSKEY = "M8 5a4 4 0 1 1 0 8a4 4 0 1 1 0-8zM11 12l8.5 8.5M16 17l2-2M18.5 19.5l2-2"
}

private val MARK_TONES = listOf(Color(0xFFFF8A4C), Color(0xFFE046B4), Color(0xFF2FD0E6), Color(0xFFC9485B), Color(0xFFF2B8C6), Color(0xFF4FB3FF))

/** The Renoa mark: an ink core with six tiles around it, one pulled out as if mid-swap. */
@Composable
fun RenoaMark(foreground: Color, size: Dp, modifier: Modifier = Modifier) {
    Canvas(modifier.size(size)) {
        val s = this.size.minDimension / 30f
        withTransform({ translate(this.size.width / 2, this.size.height / 2); scale(s, s, Offset.Zero) }) {
            for (i in 0 until 6) {
                val a = (PI / 6 + PI / 3 * i).toFloat()
                val d = if (i == 1) 10.6f else 8.6f
                drawPath(hexPath(cos(a) * d, sin(a) * d, 4.1f, 0.8f), MARK_TONES[i])
            }
            drawPath(hexPath(0f, 0f, 4.6f, 0.8f), foreground)
        }
    }
}

/**
 * A soft pool of an agent's colour behind a screen. It drifts slowly, and holds still when
 * motion is reduced.
 */
@Composable
fun Aura(color: Color, modifier: Modifier = Modifier, alpha: Float = 0.4f, offsetSeconds: Float = 0f) {
    val clock = LocalMotionClock.current
    val reduced = LocalReducedMotion.current
    Box(
        modifier.drawBehind {
            val t = if (reduced) 0f else clock.value + offsetSeconds
            // Fourteen seconds out, fourteen back, eased: the design's `app-drift`.
            val half = 1f - abs(2f * ((t % 28f) / 28f) - 1f)
            val e = RenoaEasing.EaseInOut.transform(half)
            val radius = size.minDimension / 2 * (1f + 0.12f * e)
            val centre = Offset(size.width / 2 + 24.dp.toPx() * e, size.height / 2 + 18.dp.toPx() * e)
            drawCircle(Brush.radialGradient(listOf(color.copy(alpha = alpha), color.copy(alpha = 0f)), centre, radius), radius, centre)
        },
    )
}

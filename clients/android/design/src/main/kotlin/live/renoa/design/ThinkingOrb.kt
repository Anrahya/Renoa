// Copyright 2026 Jakub Antalik (MIT; see licenses/MIT-ThinkingOrbs.txt) and
// Copyright 2026 Po Kai Lee and the Codync contributors (Apache-2.0; see licenses/Apache-2.0-Codync.txt).
// Adapted from Codync apps/android/design at ed8daf11f89398e598ef1f791abc3014ded68496.
package live.renoa.design

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

enum class ThinkingOrbState { Working, Searching, Listening, Connecting, Shaping }

/** thinking-orbs' states with their tuned dot geometry and depth shading; decorative only. */
@Composable
fun ThinkingOrb(
    modifier: Modifier = Modifier,
    state: ThinkingOrbState = ThinkingOrbState.Working,
    size: Dp = 16.dp,
    color: Color = Palette.text,
    animated: Boolean = true,
) {
    val points = with(LocalDensity.current) { size.toPx() }.toDouble()
    val reduced = LocalReducedMotion.current
    val clock = LocalMotionClock.current
    val speed = ThinkingOrbGeometry.speed(state, size.value.toDouble())
    val time = if (animated && !reduced) 0.6 + clock.value * speed else 0.6
    val scale = points / size.value
    Canvas(modifier.size(size).clearAndSetSemantics {}) {
        val frame = ThinkingOrbGeometry.frame(state, size.value.toDouble(), time)
        fun ink(white: Double, alpha: Double) = ((1 - white.coerceIn(0.0, 1.0)) * alpha).toFloat()
        for (line in frame.lines) {
            drawLine(
                color.copy(alpha = color.alpha * ink(line.white, line.alpha)),
                Offset((line.x1 * scale).toFloat(), (line.y1 * scale).toFloat()),
                Offset((line.x2 * scale).toFloat(), (line.y2 * scale).toFloat()),
                (line.width * scale).toFloat(),
            )
        }
        for (dot in frame.dots) {
            drawCircle(
                color.copy(alpha = color.alpha * ink(dot.white, dot.alpha)),
                (dot.radius * scale).toFloat(),
                Offset((dot.x * scale).toFloat(), (dot.y * scale).toFloat()),
            )
        }
    }
}

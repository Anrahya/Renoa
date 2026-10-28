package live.renoa.app.theme

import android.provider.Settings
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.State
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** Easing curves from the design: `ease` for arrivals, `spring` for a tile landing. */
object RenoaEasing {
    val Ease = CubicBezierEasing(.2f, .8f, .2f, 1f)
    val Spring = CubicBezierEasing(.34f, 1.56f, .64f, 1f)
    val EaseInOut = CubicBezierEasing(.42f, 0f, .58f, 1f)
}

/** The radii the design uses; the system's own radii are small and square, the app's are soft. */
object RenoaShapes {
    val Sheet = 28.dp
    val Card = 20.dp
    val Bubble = 22.dp
    val Control = 16.dp
}

/** One typeface, few sizes, no monospace on screen. */
object RenoaType {
    private val family = FontFamily.SansSerif
    val Display = TextStyle(fontFamily = family, fontSize = 40.sp, lineHeight = 42.sp, fontWeight = FontWeight.SemiBold, letterSpacing = (-1).sp)
    val Title = TextStyle(fontFamily = family, fontSize = 24.sp, lineHeight = 28.sp, fontWeight = FontWeight.SemiBold, letterSpacing = (-0.5).sp)
    val Heading = TextStyle(fontFamily = family, fontSize = 20.sp, lineHeight = 24.sp, fontWeight = FontWeight.SemiBold, letterSpacing = (-0.2).sp)
    val Body = TextStyle(fontFamily = family, fontSize = 16.sp, lineHeight = 24.sp)
    val Label = TextStyle(fontFamily = family, fontSize = 15.sp, lineHeight = 20.sp, fontWeight = FontWeight.SemiBold)
    val Caption = TextStyle(fontFamily = family, fontSize = 13.sp, lineHeight = 18.sp)
}

val LocalRenoaColors = staticCompositionLocalOf { ThemeName.Crimson.colors }

/** True when the person turned animations off in system settings; looping motion must stop. */
val LocalReducedMotion = compositionLocalOf { false }

/** One clock, in seconds, shared by every badge on screen so a screenful of them costs one frame loop. */
val LocalMotionClock = staticCompositionLocalOf<State<Float>> { mutableFloatStateOf(0f) }

object Renoa {
    val colors: RenoaColors
        @Composable @ReadOnlyComposable get() = LocalRenoaColors.current
}

/**
 * [reducedMotion] and [clock] default to the phone's setting and a clock that follows the display;
 * a test passes a still pair so the screen can settle.
 */
@Composable
fun RenoaTheme(
    theme: ThemeName,
    reducedMotion: Boolean = rememberReducedMotion(),
    clock: State<Float> = rememberMotionClock(running = !reducedMotion),
    content: @Composable () -> Unit,
) {
    val colors = theme.colors
    val scheme =
        darkColorScheme(
            primary = colors.primary,
            onPrimary = colors.primaryForeground,
            secondary = colors.secondary,
            onSecondary = colors.foreground,
            background = colors.background,
            onBackground = colors.foreground,
            surface = colors.card,
            onSurface = colors.foreground,
            surfaceVariant = colors.muted,
            onSurfaceVariant = colors.mutedForeground,
            outline = colors.input,
            outlineVariant = colors.border,
            error = colors.stateFailed,
        )
    val shapes =
        Shapes(
            small = RoundedCornerShape(RenoaShapes.Control),
            medium = RoundedCornerShape(RenoaShapes.Card),
            large = RoundedCornerShape(RenoaShapes.Sheet),
        )
    CompositionLocalProvider(
        LocalRenoaColors provides colors,
        LocalReducedMotion provides reducedMotion,
        LocalMotionClock provides clock,
    ) {
        MaterialTheme(colorScheme = scheme, shapes = shapes, typography = Typography()) { content() }
    }
}

@Composable
private fun rememberReducedMotion(): Boolean {
    val resolver = LocalContext.current.contentResolver
    return remember(resolver) { Settings.Global.getFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }
}

@Composable
private fun rememberMotionClock(running: Boolean): State<Float> {
    val seconds = remember { mutableFloatStateOf(0f) }
    LaunchedEffect(running) {
        if (!running) return@LaunchedEffect
        val start = withFrameNanos { it }
        while (true) {
            withFrameNanos { seconds.floatValue = (it - start) / 1_000_000_000f }
        }
    }
    return seconds
}

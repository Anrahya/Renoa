package live.renoa.app.theme

import android.content.ContentResolver
import android.database.ContentObserver
import android.graphics.Color.TRANSPARENT
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.LocalActivity
import androidx.activity.enableEdgeToEdge
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.State
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.ExperimentalTextApi
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import live.renoa.app.R

/** Easing curves from the design: `ease` for arrivals. */
object RenoaEasing {
    val Ease = CubicBezierEasing(.2f, .8f, .2f, 1f)
    val EaseInOut = CubicBezierEasing(.42f, 0f, .58f, 1f)
}

/** The design system's radii: small and square. Pills belong to state badges only. */
object RenoaShapes {
    /** `radius-lg`: plates, sheets and message bubbles. */
    val Sheet = 10.dp
    val Card = 10.dp
    val Bubble = 10.dp

    /** `radius-md`: buttons and inputs. */
    val Control = 6.dp

    /** The message composer is softer than the system's inputs: the owner found 6px too sharp for a thing you type into all day. */
    val Composer = 18.dp
}

@OptIn(ExperimentalTextApi::class)
private fun instrument(weight: Int) =
    Font(R.font.instrument_sans, FontWeight(weight), variationSettings = FontVariation.Settings(FontVariation.weight(weight)))

/** Instrument Sans for everything; IBM Plex Mono only for eyebrows and metadata, as the design system sets them. */
private val Sans = FontFamily(instrument(400), instrument(500), instrument(600))
private val Mono = FontFamily(Font(R.font.ibm_plex_mono_regular, FontWeight.Normal), Font(R.font.ibm_plex_mono_medium, FontWeight.Medium))

/** The design system's type scale (`type-*` in tokens.json), with Display sized for a phone. */
object RenoaType {
    val Display = TextStyle(fontFamily = Sans, fontSize = 40.sp, lineHeight = 40.sp, fontWeight = FontWeight.SemiBold, letterSpacing = (-0.035).em)
    val Title = TextStyle(fontFamily = Sans, fontSize = 28.sp, lineHeight = 32.sp, fontWeight = FontWeight.SemiBold, letterSpacing = (-0.02).em)
    val Heading = TextStyle(fontFamily = Sans, fontSize = 18.sp, lineHeight = 24.sp, fontWeight = FontWeight.SemiBold, letterSpacing = (-0.01).em)
    val Body = TextStyle(fontFamily = Sans, fontSize = 15.sp, lineHeight = 23.sp)
    val Label = TextStyle(fontFamily = Sans, fontSize = 14.sp, lineHeight = 20.sp, fontWeight = FontWeight.Medium)
    val Caption = TextStyle(fontFamily = Sans, fontSize = 13.sp, lineHeight = 19.sp)

    /** `type-eyebrow`: mono capitals for kickers, times and metadata. Pass the text already upper-cased. */
    val Eyebrow = TextStyle(fontFamily = Mono, fontSize = 11.5.sp, lineHeight = 12.sp, fontWeight = FontWeight.Medium, letterSpacing = 0.08.em)
}

val LocalRenoaColors = staticCompositionLocalOf { ThemeName.Graphite.colors }

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
        (if (theme.dark) darkColorScheme() else lightColorScheme()).copy(
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
        MaterialTheme(colorScheme = scheme, shapes = shapes, typography = Typography(bodyLarge = RenoaType.Body)) { content() }
    }
}

/**
 * Draws the status and navigation bars for [theme] rather than for the phone's setting, which the
 * owner may have overridden: icons that read on its ground, and the navigation bar over it.
 */
@Composable
fun SystemBars(theme: ThemeName) {
    val activity = LocalActivity.current as? ComponentActivity ?: return
    LaunchedEffect(activity, theme) {
        val scrim = theme.colors.background.copy(alpha = 0.9f).toArgb()
        if (theme.dark) {
            activity.enableEdgeToEdge(SystemBarStyle.dark(TRANSPARENT), SystemBarStyle.dark(scrim))
        } else {
            activity.enableEdgeToEdge(SystemBarStyle.light(TRANSPARENT, TRANSPARENT), SystemBarStyle.light(scrim, scrim))
        }
    }
}

/** Follows the phone's animation setting while the screen shows, so turning animations off stops the loops at once. */
@Composable
private fun rememberReducedMotion(): Boolean {
    val resolver = LocalContext.current.contentResolver
    var reduced by remember(resolver) { mutableStateOf(animationsOff(resolver)) }
    DisposableEffect(resolver) {
        val observer =
            object : ContentObserver(Handler(Looper.getMainLooper())) {
                override fun onChange(selfChange: Boolean) {
                    reduced = animationsOff(resolver)
                }
            }
        resolver.registerContentObserver(Settings.Global.getUriFor(Settings.Global.ANIMATOR_DURATION_SCALE), false, observer)
        onDispose { resolver.unregisterContentObserver(observer) }
    }
    return reduced
}

private fun animationsOff(resolver: ContentResolver) = Settings.Global.getFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f

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

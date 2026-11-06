package live.renoa.app.theme

import android.content.ContentResolver
import android.content.Context
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
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.core.content.edit
import androidx.core.graphics.drawable.toDrawable
import live.renoa.design.DesignTheme
import live.renoa.design.OverlayRoot
import live.renoa.design.Type
import live.renoa.design.bold
import live.renoa.design.medium
import live.renoa.design.monospaced
import live.renoa.design.semibold

/** Easing curves: `ease` for arrivals, as the design module's morph curve. */
object RenoaEasing {
    val Ease = CubicBezierEasing(.22f, 1f, .36f, 1f)
    val EaseInOut = CubicBezierEasing(.42f, 0f, .58f, 1f)
}

/** Corner radii, as the design module's screens round them. */
object RenoaShapes {
    /** Sheets and plates. */
    val Sheet = 22.dp

    /** Cards and list groups. */
    val Card = 16.dp

    /** Message bubbles. */
    val Bubble = 18.dp

    /** Buttons and inputs. */
    val Control = 12.dp

    /** The message composer's field. */
    val Composer = 24.dp
}

/** The app's text roles, each one of the design module's [Type] styles set in Inter. */
object RenoaType {
    val Display = Type.largeTitle.bold
    val Title = Type.title.bold
    val Heading = Type.headline
    val Body = Type.body
    val Label = Type.subheadline.medium
    val Caption = Type.footnote

    /** Kickers, times and metadata. Pass the text already upper-cased. */
    val Eyebrow = Type.caption.semibold

    /** Code in an agent's reply. */
    val Code = Type.footnote.monospaced
}

val LocalRenoaColors = staticCompositionLocalOf { ThemeName.Graphite.colors }

/** Shared with `:design` so badges, orbs and spinners obey one animation setting and clock. */
val LocalReducedMotion = live.renoa.design.LocalReducedMotion
val LocalMotionClock = live.renoa.design.LocalMotionClock

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
    DesignTheme(dark = theme.dark) {
        CompositionLocalProvider(
            LocalRenoaColors provides colors,
            LocalReducedMotion provides reducedMotion,
            LocalMotionClock provides clock,
        ) {
            // Menus and sheets draw above every screen, in the app's own window.
            OverlayRoot(content)
        }
    }
}

/** Keeps the owner's [Appearance] choice on this device, defaulting to [Appearance.System]. */
@Composable
fun rememberAppearance(): Pair<Appearance, (Appearance) -> Unit> {
    val context = LocalContext.current.applicationContext
    val prefs = remember(context) { context.getSharedPreferences("appearance", Context.MODE_PRIVATE) }
    var current by rememberSaveable {
        val saved = prefs.getString("mode", null)
        mutableStateOf(Appearance.entries.firstOrNull { it.name == saved } ?: Appearance.System)
    }
    return current to { next ->
        current = next
        prefs.edit { putString("mode", next.name) }
    }
}

/**
 * Draws the window and its status and navigation bars for [theme] rather than for the phone's
 * setting, which the owner may have overridden: its ground behind the first frame, icons that read
 * on it, and the navigation bar over it. Before the app's first frame they follow the phone.
 */
@Composable
fun SystemBars(theme: ThemeName) {
    val activity = LocalActivity.current as? ComponentActivity ?: return
    LaunchedEffect(activity, theme) {
        activity.window.setBackgroundDrawable(theme.colors.background.toArgb().toDrawable())
        val scrim = theme.colors.background.copy(alpha = 0.9f).toArgb()
        if (theme.dark) {
            activity.enableEdgeToEdge(SystemBarStyle.dark(TRANSPARENT), SystemBarStyle.dark(scrim))
        } else {
            activity.enableEdgeToEdge(SystemBarStyle.light(TRANSPARENT, TRANSPARENT), SystemBarStyle.light(scrim, scrim))
        }
    }
}

/** Follows the phone's animation setting for as long as the theme is composed, so turning animations off stops the loops at once. */
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

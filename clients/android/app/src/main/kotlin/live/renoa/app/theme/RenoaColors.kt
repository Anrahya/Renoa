package live.renoa.app.theme

import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color

/**
 * Renoa app tokens, named and valued as in `design/system/tokens.css`, so every colour traces
 * to the design system. Ink is the accent: `primary` is the ink, and there is no brand hue.
 */
@Immutable
data class RenoaColors(
    val background: Color,
    val foreground: Color,
    val foregroundSoft: Color,
    val mutedForeground: Color,
    val card: Color,
    val popover: Color,
    val secondary: Color,
    val muted: Color,
    val border: Color,
    val input: Color,
    val primary: Color,
    val primaryForeground: Color,
    val ring: Color,
    val stateRunning: Color,
    val stateFailed: Color,
)

/** The design system's two themes. Graphite is dark, Paper is light; the app follows the phone until the owner picks one. */
enum class ThemeName(val dark: Boolean, val colors: RenoaColors) {
    Graphite(
        dark = true,
        RenoaColors(
            background = Color(0xFF111312),
            foreground = Color(0xFFECEEE9),
            foregroundSoft = Color(0xFFC9CCC6),
            mutedForeground = Color(0xFF9DA29B),
            card = Color(0xFF191C1A),
            popover = Color(0xFF1E211F),
            secondary = Color(0xFF242825),
            muted = Color(0xFF202321),
            border = Color(0xFF2B2F2C),
            input = Color(0xFF646964),
            primary = Color(0xFFECEEE9),
            primaryForeground = Color(0xFF111312),
            ring = Color(0xFFECEEE9),
            stateRunning = Color(0xFF5EC29A),
            stateFailed = Color(0xFFF08770),
        ),
    ),
    Paper(
        dark = false,
        RenoaColors(
            background = Color(0xFFF5F6F3),
            foreground = Color(0xFF14161A),
            foregroundSoft = Color(0xFF464B51),
            mutedForeground = Color(0xFF5B5F66),
            card = Color(0xFFFFFFFF),
            popover = Color(0xFFFFFFFF),
            secondary = Color(0xFFE6E8E2),
            muted = Color(0xFFECEDE8),
            border = Color(0xFFD6D8D2),
            input = Color(0xFF878B85),
            primary = Color(0xFF14161A),
            primaryForeground = Color(0xFFF5F6F3),
            ring = Color(0xFF14161A),
            stateRunning = Color(0xFF1E7A57),
            stateFailed = Color(0xFFB3321E),
        ),
    ),
    ;

    /** The other theme. */
    fun next(): ThemeName = entries[(ordinal + 1) % entries.size]
}

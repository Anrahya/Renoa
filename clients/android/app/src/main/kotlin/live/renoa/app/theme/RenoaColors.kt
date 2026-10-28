package live.renoa.app.theme

import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color

/**
 * Renoa app tokens. Names follow the design system's tokens so a value can be traced to the
 * design source; each agent adds its own colours on top (see AgentLook).
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
    /** The theme's loud colour: the main action and the question only the owner can answer. */
    val signature: Color,
    val onSignature: Color,
    val stateRunning: Color,
    val stateFailed: Color,
)

/** The bold grounds. All three are dark; Crimson is the default. An agent keeps its own picture colours on top. */
enum class ThemeName(val colors: RenoaColors) {
    Midnight(
        RenoaColors(
            background = Color(0xFF0E1626),
            foreground = Color(0xFFEEF2FA),
            foregroundSoft = Color(0xFFC6D0E2),
            mutedForeground = Color(0xFF8C9AB5),
            card = Color(0xFF152036),
            popover = Color(0xFF18243C),
            secondary = Color(0xFF1E2B45),
            muted = Color(0xFF19253D),
            border = Color(0xFF26344F),
            input = Color(0xFF5A6A88),
            primary = Color(0xFFFFC145),
            primaryForeground = Color(0xFF1A1204),
            ring = Color(0xFFFFC145),
            signature = Color(0xFFFFC145),
            onSignature = Color(0xFF1A1204),
            stateRunning = Color(0xFFFFC145),
            stateFailed = Color(0xFFFF7A66),
        ),
    ),
    Ember(
        RenoaColors(
            background = Color(0xFF1E1613),
            foreground = Color(0xFFFBF1EA),
            foregroundSoft = Color(0xFFE3CFC3),
            mutedForeground = Color(0xFFB0978A),
            card = Color(0xFF29201B),
            popover = Color(0xFF2E241F),
            secondary = Color(0xFF362A24),
            muted = Color(0xFF30251F),
            border = Color(0xFF3E312A),
            input = Color(0xFF7A6558),
            primary = Color(0xFFFF7A3D),
            primaryForeground = Color(0xFF1E1613),
            ring = Color(0xFFFF7A3D),
            signature = Color(0xFFFF7A3D),
            onSignature = Color(0xFF1E1613),
            stateRunning = Color(0xFFFF7A3D),
            stateFailed = Color(0xFFFF6B6B),
        ),
    ),
    Crimson(
        RenoaColors(
            background = Color(0xFF2A0F13),
            foreground = Color(0xFFFFF0EE),
            foregroundSoft = Color(0xFFF0CFCB),
            mutedForeground = Color(0xFFC29590),
            card = Color(0xFF35161B),
            popover = Color(0xFF3A1A1F),
            secondary = Color(0xFF451E24),
            muted = Color(0xFF3D1A20),
            border = Color(0xFF4E252B),
            input = Color(0xFF8E5C60),
            primary = Color(0xFFFFB38A),
            primaryForeground = Color(0xFF2A0F13),
            ring = Color(0xFFFFB38A),
            signature = Color(0xFFFFB38A),
            onSignature = Color(0xFF2A0F13),
            stateRunning = Color(0xFFFFB38A),
            stateFailed = Color(0xFFFF8A7A),
        ),
    ),
    ;

    /** The next theme in the order above, wrapping round. */
    fun next(): ThemeName = entries[(ordinal + 1) % entries.size]
}

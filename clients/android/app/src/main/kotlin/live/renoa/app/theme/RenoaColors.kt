package live.renoa.app.theme

import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color
import live.renoa.design.Tones

/**
 * Renoa app tokens. The neutrals are the design module's [Tones], so the app and its glass share one
 * black-and-white palette; the six part pigments, which colour the agents' faces, stay as
 * `design/system/tokens.css` has them. Ink is the accent: `primary` is the ink, and there is no brand hue.
 */
@Immutable
data class RenoaColors(
    val background: Color,
    val foreground: Color,
    val mutedForeground: Color,
    val card: Color,
    val secondary: Color,
    val border: Color,
    val input: Color,
    val primary: Color,
    val primaryForeground: Color,
    val ring: Color,
    val stateFailed: Color,
    /** The six parts of an agent, as the landing page draws them: a pigment for the edge and a tint for the face. */
    val partModel: Color,
    val partModelTint: Color,
    val partLoop: Color,
    val partLoopTint: Color,
    val partContext: Color,
    val partContextTint: Color,
    val partTools: Color,
    val partToolsTint: Color,
    val partSkills: Color,
    val partSkillsTint: Color,
    val partProfile: Color,
    val partProfileTint: Color,
)

/** The two themes. Graphite is dark, Paper is light; the app follows the phone until the owner picks one. */
enum class ThemeName(val dark: Boolean, val colors: RenoaColors) {
    Graphite(
        dark = true,
        RenoaColors(
            background = Tones.Dark.background,
            foreground = Tones.Dark.text,
            mutedForeground = Tones.Dark.secondary,
            card = Tones.Dark.surface,
            secondary = Tones.Dark.bubbleAgent,
            border = Tones.Dark.border,
            input = Tones.Dark.tertiary,
            primary = Tones.Dark.accentFill,
            primaryForeground = Tones.Dark.onAccent,
            ring = Tones.Dark.accent,
            stateFailed = Tones.Dark.danger,
            partModel = Color(0xFFE27B63),
            partModelTint = Color(0xFF412F29),
            partLoop = Color(0xFFD9A24B),
            partLoopTint = Color(0xFF3F3724),
            partContext = Color(0xFFC4B654),
            partContextTint = Color(0xFF3B3B26),
            partTools = Color(0xFF52B58E),
            partToolsTint = Color(0xFF243B31),
            partSkills = Color(0xFF7298DE),
            partSkillsTint = Color(0xFF2B3541),
            partProfile = Color(0xFFA98BD2),
            partProfileTint = Color(0xFF36323F),
        ),
    ),
    Paper(
        dark = false,
        RenoaColors(
            background = Tones.Light.background,
            foreground = Tones.Light.text,
            mutedForeground = Tones.Light.secondary,
            card = Tones.Light.surface,
            secondary = Tones.Light.bubbleAgent,
            border = Tones.Light.border,
            input = Tones.Light.tertiary,
            primary = Tones.Light.accentFill,
            primaryForeground = Tones.Light.onAccent,
            ring = Tones.Light.accent,
            stateFailed = Tones.Light.danger,
            partModel = Color(0xFFC4553F),
            partModelTint = Color(0xFFF7E9E6),
            partLoop = Color(0xFFB37B22),
            partLoopTint = Color(0xFFF5EEE2),
            partContext = Color(0xFF8E8229),
            partContextTint = Color(0xFFF0EFE3),
            partTools = Color(0xFF2F8B69),
            partToolsTint = Color(0xFFE4F0EC),
            partSkills = Color(0xFF3B61AC),
            partSkillsTint = Color(0xFFE6EAF4),
            partProfile = Color(0xFF7A58A3),
            partProfileTint = Color(0xFFEEE9F3),
        ),
    ),
    ;
}

/** The appearance choice in Settings: follow the phone, or stay on Paper or Graphite. */
enum class Appearance(val label: String) {
    System("System"),
    Paper("Paper"),
    Graphite("Graphite"),
    ;

    fun resolve(systemDark: Boolean): ThemeName =
        when (this) {
            System -> if (systemDark) ThemeName.Graphite else ThemeName.Paper
            Paper -> ThemeName.Paper
            Graphite -> ThemeName.Graphite
        }
}

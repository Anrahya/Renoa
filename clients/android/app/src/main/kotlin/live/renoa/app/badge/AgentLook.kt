package live.renoa.app.badge

import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color

/**
 * What an agent looks like without a picture: six tones from its name, and a monogram on a
 * two-tone ground. Once agents have pictures, the picture's colours replace the tones here;
 * the rest of the app reads only [tones].
 */
@Immutable
class AgentLook(
    /** The six tile colours. The first is the agent's main colour: its glow, its aura, its buttons. */
    val tones: List<Color>,
    val ground: Color,
    val bloom: Color,
    val spot: Color,
    val letter: Char,
) {
    val main: Color get() = tones[0]

    companion object {
        // No greens or purples: a name lands on a warm, cyan-blue or pink hue.
        private val hues = listOf(8, 24, 40, 196, 214, 345)

        fun forName(name: String): AgentLook {
            var h = 0
            for (c in name) h = (h * 31 + c.code) % 360
            val hue = hues[h % hues.size]

            fun c(shift: Int, saturation: Int, lightness: Int) =
                Color.hsl(((hue + shift + 360) % 360).toFloat(), saturation / 100f, lightness / 100f)

            return AgentLook(
                tones = listOf(c(0, 62, 62), c(14, 55, 52), c(-12, 60, 70), c(6, 48, 44), c(-20, 55, 58), c(24, 50, 66)),
                ground = c(0, 42, 26),
                bloom = c(0, 55, 42),
                spot = c(20, 55, 50),
                letter = name.trim().firstOrNull()?.uppercaseChar() ?: '?',
            )
        }
    }
}

package live.renoa.app.badge

import androidx.compose.runtime.Immutable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp

/**
 * What an agent looks like until it has a picture: six tones and a monogram on a two-tone
 * ground. The demo agents carry the colours of the pictures drawn for them in the design;
 * any other name gets tones from its letters. Once agents have pictures, the picture's colours
 * replace the tones here; the rest of the app reads only [tones].
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

        /** The demo agents' picture colours, from the design's Face component. Renoa is orange, never purple. */
        private val pictures: Map<String, List<Long>> =
            mapOf(
                "renoa" to listOf(0xFFFF8A4C, 0xFFFFB27A, 0xFFFFE9D6, 0xFFD9622B, 0xFFFFC857, 0xFF4FB3FF),
                "kaisa" to listOf(0xFFE046B4, 0xFF2FD0E6, 0xFFFF9ACB, 0xFF1F6FD1, 0xFFFF5FA8, 0xFF0E3A7A),
                "optimus" to listOf(0xFFD12F35, 0xFF2F6BE0, 0xFFC7CDD6, 0xFF5FD3F3, 0xFFA3242A, 0xFF1B3F86),
                "nagisa" to listOf(0xFFF2B8C6, 0xFFC9A27E, 0xFFF6E3CF, 0xFFE58FA5, 0xFFA87458, 0xFFFBD7DF),
                "tifa" to listOf(0xFFC9485B, 0xFF9E1F32, 0xFFE7B595, 0xFF5A3A36, 0xFFF1ECE6, 0xFF7A1426),
            )

        fun forName(name: String): AgentLook {
            val letter = name.trim().firstOrNull()?.uppercaseChar() ?: '?'
            pictures[name.lowercase().filter { it.isLetter() }]?.let { argb ->
                val tones = argb.map { Color(it) }
                return AgentLook(tones, ground = lerp(tones[3], Color.Black, 0.55f), bloom = tones[0], spot = tones[1], letter = letter)
            }
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
                letter = letter,
            )
        }
    }
}

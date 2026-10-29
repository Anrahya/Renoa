package live.renoa.app.chat

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import live.renoa.app.theme.LocalMotionClock
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.RenoaType

/** How the working line moves: which word shows, how visible it is, and how far it has drifted. */
data class WordFrame(val index: Int, val alpha: Float, val rise: Float)

object WorkingWords {
    const val SECONDS_EACH = 2.5f

    /**
     * One word at a time: it drifts in from below, holds, and drifts out above. With
     * [reducedMotion] the first word simply stays.
     */
    fun frame(count: Int, seconds: Float, reducedMotion: Boolean): WordFrame {
        require(count > 0) { "there must be at least one word" }
        if (reducedMotion) return WordFrame(0, 1f, 0f)
        val slot = (seconds / SECONDS_EACH).toInt()
        val u = (seconds % SECONDS_EACH) / SECONDS_EACH
        return when {
            u < 0.12f -> WordFrame(slot % count, u / 0.12f, 5f * (1f - u / 0.12f))
            u < 0.88f -> WordFrame(slot % count, 1f, 0f)
            else -> WordFrame(slot % count, 1f - (u - 0.88f) / 0.12f, -5f * ((u - 0.88f) / 0.12f))
        }
    }
}

/**
 * Words that say the agent is busy, with some personality. They never say how it works; the
 * plain line under them says what it is doing.
 */
val workingWords = listOf("Tinkering", "Mulling", "Rummaging", "Simmering", "Noodling", "Untangling", "Brewing", "Puttering")

/** A playful word that changes as the agent works, and under it, in plain words, what it is doing. */
@Composable
fun WorkingLine(color: Color, doing: String?, doingColor: Color, modifier: Modifier = Modifier) {
    val clock = LocalMotionClock.current
    val reduced = LocalReducedMotion.current
    val frame = WorkingWords.frame(workingWords.size, if (reduced) 0f else clock.value, reduced)
    Column(modifier) {
        Box(Modifier.height(24.dp)) {
            Text(workingWords[frame.index] + "…", style = RenoaType.Heading.copy(fontSize = 17.sp), color = color, modifier = Modifier.offset(y = frame.rise.dp).alpha(frame.alpha))
        }
        if (doing != null) Text("$doing…", style = RenoaType.Caption.copy(fontSize = 14.sp), color = doingColor)
    }
}

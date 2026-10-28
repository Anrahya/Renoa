package live.renoa.app.chat

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import live.renoa.app.theme.LocalMotionClock
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.RenoaType
import kotlin.math.max
import kotlin.math.min

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

/** Words that say the agent is busy, with no words about how. */
val workingWords = listOf("Reading up", "Connecting the dots", "Thinking it through", "Almost there")

@Composable
fun WorkingLine(modifier: Modifier = Modifier, color: Color) {
    val clock = LocalMotionClock.current
    val reduced = LocalReducedMotion.current
    val frame = WorkingWords.frame(workingWords.size, if (reduced) 0f else clock.value, reduced)
    Box(modifier.height(24.dp)) {
        Text(workingWords[frame.index], style = RenoaType.Heading.copy(fontSize = 17.sp), color = color, modifier = Modifier.offset(y = frame.rise.dp).alpha(frame.alpha))
    }
}

/** How long a word takes to sharpen into place. */
private const val WORD_SECONDS = 0.45f

/** The alpha of word [index] given when each word arrived. */
fun wordAlpha(arrivedAt: List<Float>, index: Int, now: Float): Float =
    min(1f, max(0f, (now - arrivedAt[index]) / WORD_SECONDS))

/**
 * Text that is still arriving: each new word fades in as it lands. Nothing here is kept; the
 * caller replaces it with the whole message.
 */
@Composable
fun StreamingText(text: String, style: TextStyle, color: Color, modifier: Modifier = Modifier) {
    val clock = LocalMotionClock.current
    val reduced = LocalReducedMotion.current
    val words = remember(text) { text.split(' ').filter { it.isNotEmpty() } }
    val arrivals = remember { mutableStateListOf<Float>() }
    SideEffect {
        // A word that has already shown keeps its time; a reply that restarts shorter drops the extras.
        while (arrivals.size > words.size) arrivals.removeAt(arrivals.lastIndex)
        while (arrivals.size < words.size) arrivals += clock.value
    }
    val now = clock.value
    val annotated: AnnotatedString =
        buildAnnotatedString {
            words.forEachIndexed { i, word ->
                val a = if (reduced || i >= arrivals.size) 1f else wordAlpha(arrivals, i, now)
                withStyle(SpanStyle(color = color.copy(alpha = color.alpha * a))) { append(word) }
                if (i < words.lastIndex) append(' ')
            }
        }
    Text(annotated, style = style, modifier = modifier)
}

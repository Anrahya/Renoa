package live.renoa.app.chat

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.SemanticsPropertyKey
import androidx.compose.ui.semantics.SemanticsPropertyReceiver
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.flow.first
import live.renoa.app.theme.LocalMotionClock
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.RenoaType
import kotlin.math.ceil
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

/**
 * How fast a reply is revealed. It runs at least at [BASE_CHARS_PER_SECOND]. When words arrive
 * faster, it speeds up so each character shows within [MAX_LAG_SECONDS] of arriving (plus one
 * frame), and it never slows down while behind, so that limit holds for every character. Once the
 * reply is whole, the rest shows within [FINISH_SECONDS]. A reply already done is never held back.
 */
object Reveal {
    const val BASE_CHARS_PER_SECOND = 90f
    const val MAX_LAG_SECONDS = 0.3f
    const val FINISH_SECONDS = 0.2f

    /** How far the reveal has got, and how fast it is going. The speed resets once it catches up. */
    data class Progress(val shown: Float = 0f, val speed: Float = 0f)

    /** Where the reveal is after [dt] seconds, with [arrived] characters in all. */
    fun step(progress: Progress, arrived: Int, dt: Float, complete: Boolean): Progress {
        val backlog = arrived - progress.shown
        if (backlog <= 0f) return Progress(arrived.toFloat(), 0f)
        var speed = maxOf(progress.speed, BASE_CHARS_PER_SECOND, backlog / MAX_LAG_SECONDS)
        if (complete) speed = max(speed, backlog / FINISH_SECONDS)
        val shown = min(arrived.toFloat(), progress.shown + speed * dt)
        return Progress(shown, if (shown >= arrived) 0f else speed)
    }

    /** The edge is soft: the last [FADE_CHARS] shown characters ramp from clear to faint. */
    const val FADE_CHARS = 14f

    fun alpha(index: Int, shown: Float): Float = ((shown - index) / FADE_CHARS).coerceIn(0f, 1f)

    /**
     * Where the reveal stops. A whole reply runs [FADE_CHARS] past its end so its last words
     * come fully clear; a reply still arriving keeps a soft edge on its newest words.
     */
    fun end(length: Int, complete: Boolean): Int = length + if (complete) FADE_CHARS.toInt() else 0
}

/**
 * An agent's reply, revealed smoothly. The whole text is laid out from the start and only its
 * not-yet-shown end is transparent, so lines never jump. [complete] is false while the reply
 * is still arriving. With [animate] false, or reduced motion, it shows at once.
 */
@Composable
fun RevealText(text: String, complete: Boolean, animate: Boolean, style: TextStyle, color: Color, modifier: Modifier = Modifier) {
    val reduced = LocalReducedMotion.current
    val end = Reveal.end(text.length, complete)
    var progress by remember { mutableStateOf(Reveal.Progress(if (animate) 0f else end.toFloat())) }
    val target by rememberUpdatedState(end)
    val whole by rememberUpdatedState(complete)
    // One loop for the life of the text. New words do not restart it, so none of them costs a frame.
    LaunchedEffect(Unit) {
        while (true) {
            snapshotFlow { progress.shown < target }.first { it }
            var last = withFrameNanos { it }
            while (progress.shown < target) {
                val now = withFrameNanos { it }
                progress = Reveal.step(progress, target, (now - last) / 1_000_000_000f, whole)
                last = now
            }
        }
    }
    val edge = if (reduced) end.toFloat() else progress.shown
    val annotated =
        buildAnnotatedString {
            val solid = max(0, (edge - Reveal.FADE_CHARS).toInt()).coerceAtMost(text.length)
            val hidden = ceil(edge).toInt().coerceIn(solid, text.length)
            withStyle(SpanStyle(color = color)) { append(text, 0, solid) }
            for (i in solid until hidden) withStyle(SpanStyle(color = color.copy(alpha = color.alpha * Reveal.alpha(i, edge)))) { append(text[i]) }
            withStyle(SpanStyle(color = Color.Transparent)) { append(text, hidden, text.length) }
        }
    Text(annotated, style = style, modifier = modifier.semantics { revealed = min(edge, text.length.toFloat()).toInt() })
}

/** How many characters of a reply are showing, for tests: the whole text is always laid out. */
val Revealed = SemanticsPropertyKey<Int>("Revealed")
var SemanticsPropertyReceiver.revealed by Revealed

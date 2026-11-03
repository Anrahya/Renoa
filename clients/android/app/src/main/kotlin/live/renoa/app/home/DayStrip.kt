package live.renoa.app.home

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import live.renoa.app.agents.clockTime
import live.renoa.app.badge.AgentHue
import live.renoa.app.theme.LocalMotionClock
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaType
import java.time.Duration
import java.time.LocalTime
import java.time.ZonedDateTime
import kotlin.math.PI
import kotlin.math.cos

/** How long a run to come counts down on its ring before it is due. */
private const val COUNTDOWN_SECONDS = 600f

/** How long the strip keeps naming a mark after the owner lets go. */
private const val LINGER_MS = 1_200L

private val HOUR_LABELS = listOf(LocalTime.of(6, 0) to "6 AM", LocalTime.NOON to "NOON", LocalTime.of(18, 0) to "6 PM", LocalTime.MAX to "12 AM")

/**
 * The day from 6 am to midnight, with the hour either side of now drawn wide. Above the bar, what
 * agents left the owner: a question rides high and bobs, a failed routine is a ring in the failure
 * colour, and marks too close to tell apart show as one with a count. Below it, each routine's run:
 * a dot once it ran, a ring while it is to come that fills in its last ten minutes. Dragging along
 * it names the nearest mark in the line underneath and tells [onLit] whose it is.
 */
@Composable
internal fun DayStrip(state: HomeUiState, now: ZonedDateTime, onLit: (String?) -> Unit, onRoutines: () -> Unit, modifier: Modifier = Modifier) {
    val colors = Renoa.colors
    val clock = LocalMotionClock.current
    val reduced = LocalReducedMotion.current
    val lens = remember(now) { DayLens(now.toLocalTime()) }
    val marks = remember(state.roster, state.feed, state.routines, now) { stripMarks(state.roster, state.feed, state.routines, now) }
    val measurer = rememberTextMeasurer()
    val inks = remember(state.roster, colors) { state.roster.associate { it.id to AgentHue.of(it.id, state.roster).face(colors) } }
    val hours = remember(measurer, colors) { HOUR_LABELS.map { (time, label) -> time to measurer.measure(label, RenoaType.Eyebrow.copy(color = colors.mutedForeground)) } }
    var scrub by remember { mutableStateOf<Float?>(null) }
    var released by remember { mutableStateOf(0) }
    var dragging by remember { mutableStateOf(false) }
    val lit by rememberUpdatedState(onLit)
    val within = 0.04f
    val picked = scrub?.let { nearest(marks, lens, it, within) }
    LaunchedEffect(picked?.agentId, scrub == null) { lit(picked?.agentId) }
    LaunchedEffect(released) {
        if (released > 0) {
            delay(LINGER_MS)
            // A drag begun meanwhile keeps its mark.
            if (!dragging) scrub = null
        }
    }
    val words =
        when {
            // Each mark a sentence of its own, with no second stop after a question or a note's own.
            marks.isNotEmpty() -> marks.joinToString(" ", prefix = "Today. ") { mark -> mark.words.trimEnd().let { if (it.isEmpty() || it.last() in ".?!") it else "$it." } }
            // Nothing can be said of the day before Renoa has said what runs in it.
            state.known -> "Today. Nothing runs today."
            else -> "Today."
        }
    val line =
        when {
            scrub == null -> stripLine(marks, state.routines, now)
            picked != null -> "${clockTime(picked.at.toLocalTime())} · ${picked.words}"
            else -> "${clockTime(LocalTime.ofSecondOfDay((lens.hourAt(scrub!!) * 3600).toLong().coerceIn(0, 86_399)))} · Nothing then"
        }

    Column(modifier.fillMaxWidth().semantics { contentDescription = words }) {
        Canvas(
            Modifier
                .fillMaxWidth()
                .height(66.dp)
                .pointerInput(Unit) {
                    detectHorizontalDragGestures(
                        onDragStart = {
                            dragging = true
                            scrub = (it.x / size.width).coerceIn(0f, 1f)
                        },
                        onDragEnd = {
                            dragging = false
                            released++
                        },
                        onDragCancel = {
                            dragging = false
                            released++
                        },
                    ) { change, _ ->
                        change.consume()
                        scrub = (change.position.x / size.width).coerceIn(0f, 1f)
                    }
                },
        ) {
            val w = size.width
            fun x(time: LocalTime) = w * lens.at(time)
            val bar = 22.dp.toPx()
            val runs = 42.dp.toPx()
            val labels = runs + 10.dp.toPx()
            // The magnified hours, faintly.
            drawRoundRect(colors.secondary.copy(alpha = 0.5f), Offset(w * lens.atHour(lens.from), 0f), Size(w * (lens.atHour(lens.to) - lens.atHour(lens.from)), labels - 2.dp.toPx()), CornerRadius(6.dp.toPx()))
            val nowX = x(now.toLocalTime())
            val thick = 6.dp.toPx()
            drawRoundRect(colors.secondary, Offset(0f, bar - thick / 2), Size(w, thick), CornerRadius(thick / 2))
            // The day so far warms from the morning's ochre to the evening's clay.
            val sofar = Brush.horizontalGradient(listOf(colors.partLoop.copy(alpha = 0.5f), colors.partModel.copy(alpha = 0.85f)), endX = w)
            drawRoundRect(sofar, Offset(0f, bar - thick / 2), Size(nowX, thick), CornerRadius(thick / 2))
            drawLine(colors.border, Offset(0f, runs), Offset(w, runs), 1.5.dp.toPx())

            for (mark in marks.filter { !it.kind.left }) {
                val at = Offset(x(mark.at.toLocalTime()), runs)
                val ink = inks[mark.agentId] ?: colors.foreground
                drawRun(mark, at, ink, now, colors.background, colors.stateFailed, colors.mutedForeground)
            }
            val bob = if (reduced) 0f else (0.5f - 0.5f * cos(2 * PI.toFloat() * clock.value / 1.6f)) * 3.dp.toPx()
            for (group in clustered(marks.filter { it.kind.left }, { x(it.at.toLocalTime()) }, 8.dp.toPx())) {
                val lead = group.minBy { it.kind.ordinal }
                val asks = group.any { it.kind == MarkKind.Ask }
                val at = Offset(group.map { x(it.at.toLocalTime()) }.average().toFloat(), (if (asks) 6.dp.toPx() - bob else 10.dp.toPx()))
                val ink = if (lead.kind == MarkKind.Problem) colors.stateFailed else inks[lead.agentId] ?: colors.foreground
                drawCircle(colors.background, (if (group.size > 1) 9 else 6).dp.toPx(), at)
                when {
                    group.size > 1 -> {
                        drawCircle(ink, 7.5.dp.toPx(), at)
                        val count = measurer.measure("${group.size}", RenoaType.Eyebrow.copy(color = colors.background))
                        drawText(count, topLeft = at - Offset(count.size.width / 2f, count.size.height / 2f))
                    }
                    lead.kind == MarkKind.Problem -> drawCircle(ink, 4.dp.toPx(), at, style = Stroke(1.6.dp.toPx()))
                    else -> drawCircle(ink, (if (asks) 5 else 4).dp.toPx(), at)
                }
            }

            val breath = if (reduced) 1f else 0.6f + 0.4f * cos(2 * PI.toFloat() * clock.value / 2.8f)
            drawLine(colors.foreground.copy(alpha = breath), Offset(nowX, bar - 10.dp.toPx()), Offset(nowX, runs + 6.dp.toPx()), 2.dp.toPx(), StrokeCap.Round)
            scrub?.let { drawLine(colors.mutedForeground, Offset(w * it, 0f), Offset(w * it, size.height), 1.dp.toPx()) }

            for ((time, text) in hours) {
                val left = (x(time) - text.size.width / 2f).coerceIn(0f, w - text.size.width)
                drawText(text, topLeft = Offset(left, labels))
            }
        }
        // At rest the line says what runs next, and opens the routines.
        Row(
            Modifier.fillMaxWidth().heightIn(min = 40.dp).clickable(enabled = scrub == null, onClickLabel = "Open routines", onClick = onRoutines),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(line, Modifier.weight(1f), style = RenoaType.Caption, color = if (scrub == null) colors.mutedForeground else colors.foreground, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (scrub == null) Text("\u203a", Modifier.padding(start = 8.dp), style = RenoaType.Label, color = colors.foregroundSoft)
        }
    }
}

/** A run on the lower line: a dot once it ran, a ring in the failure colour if it failed, a ring that fills while it comes due. */
private fun DrawScope.drawRun(mark: StripMark, at: Offset, ink: Color, now: ZonedDateTime, ground: Color, failed: Color, muted: Color) {
    drawCircle(ground, 6.dp.toPx(), at)
    when (mark.kind) {
        MarkKind.Ran -> drawCircle(ink, 3.5.dp.toPx(), at)
        MarkKind.Failed -> drawCircle(failed, 4.dp.toPx(), at, style = Stroke(1.6.dp.toPx()))
        MarkKind.Skipped -> drawCircle(muted, 2.dp.toPx(), at)
        else -> {
            val left = Duration.between(now, mark.at).seconds.toFloat()
            val filled = (1 - left / COUNTDOWN_SECONDS).coerceIn(0f, 1f)
            val r = 4.dp.toPx()
            drawCircle(ink.copy(alpha = 0.5f), r, at, style = Stroke(1.4.dp.toPx()))
            if (filled > 0f) drawArc(ink, -90f, 360f * filled, false, at - Offset(r, r), Size(r * 2, r * 2), style = Stroke(2.dp.toPx()))
        }
    }
}

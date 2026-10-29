package live.renoa.app.home

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import live.renoa.app.agents.RunOutcome
import live.renoa.app.agents.clockTime
import live.renoa.app.badge.AgentHue
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.RenoaMark
import java.time.ZonedDateTime

/**
 * Home is the owner's day with their agents: the day so far and what runs in it, the main agent
 * with everyone round her, one thing at a time handed over, and a bar that opens the chats,
 * routines and what's coming up.
 */
@Composable
fun HomeScreen(
    viewModel: HomeViewModel,
    onOpenChat: (String) -> Unit,
    onNewAgent: () -> Unit,
    onChangeColours: () -> Unit,
    now: () -> ZonedDateTime = { ZonedDateTime.now() },
) {
    val state by viewModel.state.collectAsState()
    // Read once per visit: the first visit assembles, later ones show the tiles in place.
    val assemble = remember { !viewModel.assembled }
    LaunchedEffect(Unit) { viewModel.markAssembled() }
    val time by produceState(now()) {
        while (true) {
            delay(60_000L - value.second * 1_000L)
            value = now()
        }
    }
    val next = nextWords(state.routines, time)
    val colors = Renoa.colors
    BackHandler(enabled = state.page != null) { viewModel.close() }

    Box(Modifier.fillMaxSize().background(colors.background)) {
        // Under an open page, Home is hidden from accessibility, so only the page is read.
        Column(Modifier.fillMaxSize().safeDrawingPadding().then(if (state.page != null) Modifier.clearAndSetSemantics { } else Modifier)) {
            Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(
                    Modifier
                        .size(48.dp)
                        .clickable(role = Role.Button, onClick = onChangeColours)
                        .semantics { contentDescription = "Renoa, change colours" },
                    contentAlignment = Alignment.CenterStart,
                ) { RenoaMark(colors.foreground, 26.dp) }
                Spacer(Modifier.weight(1f))
                Text(clockWords(time).uppercase(), style = RenoaType.Eyebrow, color = colors.mutedForeground)
            }
            Text(greeting(time.hour), Modifier.padding(horizontal = 20.dp), style = RenoaType.Title, color = colors.foreground)
            DayStrip(state, time, { viewModel.open(HomePage.Routines) }, Modifier.padding(start = 20.dp, end = 20.dp, top = 14.dp))
            Constellation(state, assemble, onOpenChat, onNewAgent, { viewModel.open(HomePage.Chats) }, Modifier.weight(1f).padding(top = 8.dp))
            TalkLine(state, onOpenChat)
            Relay(state, next, viewModel::show, onOpenChat, viewModel::answer, Modifier.padding(horizontal = 16.dp))
            state.notice?.takeIf { state.page == null }?.let { Text(it, Modifier.padding(start = 24.dp, top = 6.dp), style = RenoaType.Caption, color = colors.stateFailed) }
            Dock(state, next, { viewModel.open(HomePage.Chats) }, Modifier.padding(start = 12.dp, end = 12.dp, top = 10.dp, bottom = 8.dp))
        }
        AnimatedVisibility(
            visible = state.page != null,
            enter = slideInVertically(tween(300)) { it / 6 } + fadeIn(tween(300)),
            exit = slideOutVertically(tween(200)) { it / 6 } + fadeOut(tween(200)),
        ) {
            HomeDrawer(state, time, viewModel::open, viewModel::close, onOpenChat, viewModel::answer, onNewAgent, viewModel::switch)
        }
    }
}

/**
 * The day from 6 am to midnight: the part gone by is filled, a line marks now, and each routine
 * that ran today or still runs today is a dot in its agent's colour, hollow while it is to come.
 * Tapping it opens the routines.
 */
@Composable
private fun DayStrip(state: HomeUiState, now: ZonedDateTime, onOpen: () -> Unit, modifier: Modifier = Modifier) {
    val colors = Renoa.colors
    val marks = dayMarks(state.routines, now)
    val words =
        marks.joinToString(prefix = "Today. ", separator = ". ") { mark ->
            val at = clockTime(mark.at.toLocalTime())
            when (mark.ran) {
                null -> "${mark.routine.name} at $at"
                RunOutcome.Failed -> "${mark.routine.name} failed at $at"
                RunOutcome.Done, RunOutcome.Skipped -> "${mark.routine.name} ran at $at"
            }
        }.ifEmpty { "Today. Nothing runs today." }
    Column(modifier.fillMaxWidth().clickable(role = Role.Button, onClickLabel = "Open routines", onClick = onOpen).semantics { contentDescription = words }) {
        Canvas(Modifier.fillMaxWidth().height(22.dp)) {
            val y = size.height / 2
            val bar = 4.dp.toPx()
            val corner = CornerRadius(bar / 2)
            drawRoundRect(colors.secondary, Offset(0f, y - bar / 2), Size(size.width, bar), corner)
            val nowX = size.width * stripAt(now.toLocalTime())
            drawRoundRect(colors.input, Offset(0f, y - bar / 2), Size(nowX, bar), corner)
            for (mark in marks) {
                val x = size.width * stripAt(mark.at.toLocalTime())
                val ink =
                    when (mark.ran) {
                        RunOutcome.Failed -> colors.stateFailed
                        else -> state.agent(mark.routine.agentId)?.let { AgentHue.of(it.id, state.roster).face(colors) } ?: colors.foreground
                    }
                drawCircle(colors.background, 6.dp.toPx(), Offset(x, y))
                if (mark.ran == null) drawCircle(ink, 4.dp.toPx(), Offset(x, y), style = Stroke(1.6.dp.toPx())) else drawCircle(ink, 4.5.dp.toPx(), Offset(x, y))
            }
            drawLine(colors.foreground, Offset(nowX, 1.dp.toPx()), Offset(nowX, size.height - 1.dp.toPx()), 1.5.dp.toPx())
        }
        Row(Modifier.fillMaxWidth().padding(top = 2.dp), horizontalArrangement = Arrangement.SpaceBetween) {
            listOf("6 am", "Noon", "6 pm", "12 am").forEach { Text(it, style = RenoaType.Eyebrow, color = colors.mutedForeground) }
        }
    }
}

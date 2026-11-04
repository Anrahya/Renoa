package live.renoa.app.home

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import live.renoa.app.agents.Routine
import live.renoa.app.badge.Badge
import live.renoa.app.badge.hexPath
import live.renoa.app.theme.Renoa
import live.renoa.design.CardSection
import live.renoa.design.DesignSwitch
import live.renoa.design.Motion
import live.renoa.design.Palette
import live.renoa.design.SegmentedChoice
import live.renoa.design.Type
import live.renoa.design.semibold
import java.time.ZonedDateTime

/** The Renoa tab: its pages side by side under one segmented choice, [page] showing. */
@Composable
internal fun RenoaTab(state: HomeUiState, now: ZonedDateTime, page: HomePage, onPage: (HomePage) -> Unit, onSwitch: (String, Boolean) -> Unit, top: Dp, bottom: Dp) {
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = top + 64.dp, bottom = bottom), verticalArrangement = Arrangement.spacedBy(20.dp)) {
        item(key = "segments") { SegmentedChoice(page, HomePage.entries.map { it to it.title }, onPage) }
        item(key = "page") {
            AnimatedContent(page, transitionSpec = { fadeIn(Motion.fade()) togetherWith fadeOut(Motion.fade()) }, label = "Renoa page") { shown ->
                when (shown) {
                    HomePage.Routines -> Routines(state, now, onSwitch)
                    HomePage.Upcoming -> Upcoming(state, now)
                }
            }
        }
        state.notice?.let { notice -> item(key = "notice") { Text(notice, style = Type.footnote, color = Palette.danger) } }
    }
}

/** Every routine, with what it does next and a switch to turn it on or off. */
@Composable
private fun Routines(state: HomeUiState, now: ZonedDateTime, onSwitch: (String, Boolean) -> Unit) {
    if (state.routines.isEmpty()) return Empty("No routines yet. Ask an agent to check on something every day, and it shows here.")
    CardSection {
        for (routine in state.routines) {
            row {
                val on = state.isOn(routine)
                DesignSwitch(on, { onSwitch(routine.id, it) }, enabled = routine.id !in state.switching) {
                    RoutineBadge(state, routine, 40.dp)
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(routine.name, style = Type.body.semibold, color = Palette.text)
                        Text(listOfNotNull(state.agent(routine.agentId)?.name, routine.schedule).joinToString(" · "), style = Type.footnote, color = Palette.secondary)
                        Text(routineStatus(routine.copy(on = on, next = routine.next.takeIf { on }), now), style = Type.footnote, color = Palette.secondary)
                    }
                }
            }
        }
    }
}

/** What runs next, grouped by when. */
@Composable
private fun Upcoming(state: HomeUiState, now: ZonedDateTime) {
    val groups = upcoming(state.routines, now)
    if (groups.isEmpty()) return Empty("Nothing is coming up. The routines you turn on show here.")
    Column(verticalArrangement = Arrangement.spacedBy(24.dp)) {
        for (group in groups) {
            CardSection(title = group.horizon.heading) {
                for ((routine, time) in group.runs) {
                    row {
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
                            RoutineBadge(state, routine, 34.dp)
                            Column(Modifier.weight(1f)) {
                                Text(routine.name, style = Type.body, color = Palette.text)
                                state.agent(routine.agentId)?.let { Text(it.name, style = Type.footnote, color = Palette.secondary) }
                            }
                            Text(time, style = Type.subheadline, color = Palette.secondary)
                        }
                    }
                }
            }
        }
    }
}

/** The tile of the agent that runs [routine], or an empty socket while that agent is not on the roster. */
@Composable
private fun RoutineBadge(state: HomeUiState, routine: Routine, size: Dp) {
    val agent = state.agent(routine.agentId)
    if (agent == null) EmptySocket(size) else Badge(agent, state.roster, size, label = "")
}

/** An empty tile's socket. */
@Composable
private fun EmptySocket(size: Dp) {
    val colors = Renoa.colors
    Canvas(Modifier.size(size)) {
        val s = this.size.minDimension / 84f
        withTransform({ translate(this.size.width / 2, this.size.height / 2); scale(s, s, Offset.Zero) }) {
            drawPath(hexPath(0f, 0f, 36f, 4f), colors.border, style = Stroke(1.5f))
        }
    }
}

@Composable
private fun Empty(words: String) {
    Text(words, Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 24.dp), style = Type.body, color = Palette.secondary)
}

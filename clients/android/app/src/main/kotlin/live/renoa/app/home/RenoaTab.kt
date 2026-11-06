// Portions copyright 2026 Po Kai Lee and the Codync contributors.
// Licensed under the Apache License, Version 2.0; see licenses/Apache-2.0-Codync.txt.
// Adapted from Codync apps/android `StateScreen.kt` at ed8daf11f89398e598ef1f791abc3014ded68496.
package live.renoa.app.home

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
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
import live.renoa.design.CardSection
import live.renoa.design.DesignSwitch
import live.renoa.design.Motion
import live.renoa.design.Palette
import live.renoa.design.ScreenHeader
import live.renoa.design.SegmentedChoice
import live.renoa.design.ThinkingOrb
import live.renoa.design.ThinkingOrbState
import live.renoa.design.Type
import live.renoa.design.semibold
import java.time.ZonedDateTime

/** The Renoa tab: header and segmented choice over its pages, [page] showing, each fading in for the last. */
@Composable
internal fun RenoaTab(state: HomeUiState, now: ZonedDateTime, page: HomePage, act: RenoaActions, bottom: Dp) {
    Column(Modifier.fillMaxSize().background(Palette.background).statusBarsPadding()) {
        ScreenHeader(title = { Text("Renoa", style = Type.body.semibold, color = Palette.text) })
        SegmentedChoice(
            page,
            HomePage.entries.map { it to it.title },
            act.open,
            Modifier.padding(horizontal = 16.dp).padding(bottom = 8.dp),
        )
        AnimatedContent(
            page,
            Modifier.weight(1f),
            transitionSpec = { fadeIn(Motion.fade()) togetherWith fadeOut(Motion.fade()) },
            label = "Renoa page",
        ) { shown ->
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(start = 18.dp, end = 18.dp, top = 16.dp, bottom = bottom),
                verticalArrangement = Arrangement.spacedBy(20.dp),
            ) {
                item(key = shown.name) {
                    when (shown) {
                        HomePage.Routines -> Routines(state, now, act.switchRoutine)
                        HomePage.Memory -> MemoryPage(state) { act.retry(HomePage.Memory) }
                        HomePage.Apps -> AppsPage(state, act.switchApp, act.ask) { act.retry(HomePage.Apps) }
                    }
                }
            }
        }
    }
}

/**
 * What the Renoa tab does with the owner's taps: turn to a page, read one again, switch a routine
 * or an app, or open a chat to ask for one.
 */
internal class RenoaActions(
    val open: (HomePage) -> Unit,
    val retry: (HomePage) -> Unit,
    val switchRoutine: (String, Boolean) -> Unit,
    val switchApp: (String, Boolean) -> Unit,
    val ask: (String) -> Unit,
)

/** Every routine, soonest first, with what it does next and a switch to turn it on or off. */
@Composable
private fun Routines(state: HomeUiState, now: ZonedDateTime, onSwitch: (String, Boolean) -> Unit) {
    if (state.routines.isEmpty()) return Empty("No routines yet. Ask an agent to check on something every day, and it shows here.")
    CardSection {
        for (routine in soonestFirst(state.routines)) {
            row {
                val on = state.isOn(routine)
                DesignSwitch(on, { onSwitch(routine.id, it) }, enabled = routine.id !in state.switching) {
                    RoutineBadge(state, routine, 34.dp)
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(routine.name, style = Type.subheadline.semibold, color = Palette.text)
                        Text(listOfNotNull(state.agent(routine.agentId)?.name, routine.schedule).joinToString(" · "), style = Type.footnote, color = Palette.secondary)
                        Text(routineStatus(routine.copy(on = on, next = routine.next.takeIf { on }), now), style = Type.footnote, color = Palette.secondary)
                    }
                }
            }
        }
    }
}

/** The tile of the agent that runs [routine], or an empty outline while that agent is not on the roster. */
@Composable
private fun RoutineBadge(state: HomeUiState, routine: Routine, size: Dp) {
    val agent = state.agent(routine.agentId)
    if (agent == null) EmptyBadgeOutline(size) else Badge(agent, state.roster, size, label = "")
}

@Composable
private fun EmptyBadgeOutline(size: Dp) {
    val border = Palette.border
    Canvas(Modifier.size(size)) {
        val s = this.size.minDimension / 84f
        withTransform({ translate(this.size.width / 2, this.size.height / 2); scale(s, s, Offset.Zero) }) {
            drawPath(hexPath(0f, 0f, 36f, 4f), border, style = Stroke(1.5f))
        }
    }
}

@Composable
internal fun LoadingPage(words: String) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 24.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        ThinkingOrb(state = ThinkingOrbState.Connecting, size = 18.dp, color = Palette.secondary)
        Text(words, style = Type.body, color = Palette.secondary)
    }
}

@Composable
internal fun Empty(words: String) {
    Text(words, Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 24.dp), style = Type.body, color = Palette.secondary)
}

// Portions copyright 2026 Po Kai Lee and the Codync contributors.
// Licensed under the Apache License, Version 2.0; see licenses/Apache-2.0-Codync.txt.
// The tab layers and the bars over them are adapted from Codync apps/android `MainTabs.kt` and
// `BotListView.kt` at ed8daf11f89398e598ef1f791abc3014ded68496.
package live.renoa.app.home

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Text
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex
import com.kyant.shapes.Capsule
import com.kyant.shapes.RoundedRectangle
import kotlinx.coroutines.delay
import live.renoa.app.ui.LinkLine
import live.renoa.design.Edge
import live.renoa.design.GlassStage
import live.renoa.design.GlassTabBar
import live.renoa.design.Palette
import live.renoa.design.PrimaryButton
import live.renoa.design.ScrollEdge
import live.renoa.design.Symbol
import live.renoa.design.TabItem
import live.renoa.design.Type
import live.renoa.design.glass
import live.renoa.design.rememberPress
import live.renoa.design.semibold
import java.time.ZonedDateTime

/** Home's two tabs: the agents, and Renoa's routines. */
private enum class HomeTab { Agents, Renoa }

/**
 * Home is the owner's agents, with the day on top, and a tab of Renoa's routines beside them. The
 * bars float as glass over both. Until it is [HomeUiState.known] who is here, Home says it is
 * checking in. The Renoa tab shows while a [HomeUiState.page] is open.
 */
@Composable
fun HomeScreen(
    viewModel: HomeViewModel,
    onOpenChat: (String) -> Unit,
    onNewAgent: () -> Unit,
    onChangeColours: () -> Unit,
    now: () -> ZonedDateTime = { ZonedDateTime.now() },
    onSignUp: () -> Unit = {},
) {
    val state by viewModel.state.collectAsState()
    LaunchedEffect(state.signUp) { if (state.signUp) onSignUp() }
    val time by produceState(now()) {
        while (true) {
            delay(60_000L - value.second * 1_000L)
            value = now()
        }
    }
    var lit by remember { mutableStateOf<String?>(null) }
    val tab = if (state.page != null) HomeTab.Renoa else HomeTab.Agents
    val act =
        TalkActions(
            open = { talk ->
                talk.note?.let { viewModel.seen(it.id) }
                onOpenChat(talk.agent.id)
            },
            answer = viewModel::answer,
            runAgain = viewModel::runAgain,
        )
    BackHandler(enabled = state.page != null) { viewModel.close() }

    val top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()
    val bottom = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()
    val barSpace = bottom + 62.dp + 24.dp
    GlassStage(
        Modifier.fillMaxSize().background(Palette.background),
        chrome = {
            ScrollEdge(Edge.Top, top + 64.dp)
            TopBar(state, tab, onChangeColours, onNewAgent)
            Box(Modifier.align(Alignment.BottomCenter), contentAlignment = Alignment.BottomCenter) {
                ScrollEdge(Edge.Bottom, barSpace)
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    // A refusal stays in sight above the bar, however far the list scrolled.
                    state.notice?.let { Text(it, Modifier.padding(horizontal = 24.dp, vertical = 8.dp), style = Type.footnote, color = Palette.danger) }
                    if (state.known) {
                    GlassTabBar(
                        tab,
                        listOf(TabItem(HomeTab.Agents, "Agents", "bubble.left.and.bubble.right.fill", 33.dp), TabItem(HomeTab.Renoa, "Renoa", "hexagon.fill")),
                        { picked -> if (picked == HomeTab.Agents) viewModel.close() else viewModel.open(HomePage.Routines) },
                        Modifier.padding(bottom = maxOf(bottom - 4.dp, 12.dp)),
                    )
                    }
                }
            }
        },
    ) {
        Box(Modifier.fillMaxSize().tabLayer(tab == HomeTab.Agents)) { AgentList(state, time, lit, { lit = it }, act, onNewAgent, viewModel, top, barSpace) }
        Box(Modifier.fillMaxSize().tabLayer(tab == HomeTab.Renoa)) {
            RenoaTab(state, time, viewModel::switch, top, barSpace)
        }
    }
}

/** The bar over Home: the owner's capsule, which changes the colours, the line while Renoa is out of reach, and a way to meet someone new. */
@Composable
private fun TopBar(state: HomeUiState, tab: HomeTab, onChangeColours: () -> Unit, onNewAgent: () -> Unit) {
    Row(Modifier.fillMaxWidth().statusBarsPadding().padding(top = 6.dp).padding(horizontal = 16.dp).height(44.dp), verticalAlignment = Alignment.CenterVertically) {
        val (source, pressed) = rememberPress()
        Box(
            Modifier
                .minimumInteractiveComponentSize()
                .size(44.dp)
                .glass(Capsule(), pressed = pressed)
                .clickable(source, null, role = Role.Button, onClick = onChangeColours)
                .semantics { contentDescription = "Change colours" },
            contentAlignment = Alignment.Center,
        ) {
            Box(Modifier.size(30.dp).background(Palette.accentDim, Capsule()), contentAlignment = Alignment.Center) {
                val initial = state.ownerName?.trim()?.take(1)?.uppercase()
                if (initial.isNullOrEmpty()) Symbol("person.fill", size = 18.dp, tint = Palette.text) else Text(initial, style = Type.subheadline.semibold, color = Palette.text)
            }
        }
        Box(Modifier.weight(1f), contentAlignment = Alignment.Center) {
            if (tab == HomeTab.Renoa) Text("Renoa", style = Type.body.semibold, color = Palette.text) else LinkLine(state.link)
        }
        val (plus, plusPressed) = rememberPress()
        Box(
            Modifier
                .minimumInteractiveComponentSize()
                .size(44.dp)
                .glass(Capsule(), pressed = plusPressed)
                .clickable(plus, null, role = Role.Button, onClick = onNewAgent)
                .semantics { contentDescription = "Meet someone new" },
            contentAlignment = Alignment.Center,
        ) { Symbol("plus", size = 30.dp, tint = Palette.text) }
    }
}

/** The day on top, then the main agent, everyone with something to say, and the quiet ones. */
@Composable
private fun AgentList(
    state: HomeUiState,
    now: ZonedDateTime,
    lit: String?,
    onLit: (String?) -> Unit,
    act: TalkActions,
    onNewAgent: () -> Unit,
    viewModel: HomeViewModel,
    top: Dp,
    bottom: Dp,
) {
    val talks = state.talks
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = top + 56.dp, bottom = bottom)) {
        if (state.known && talks.main == null) {
            item(key = "first") {
                Column(Modifier.fillMaxWidth().padding(vertical = 40.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(14.dp)) {
                    Text("Nobody is here yet", style = Type.title3.semibold, color = Palette.text)
                    Text("Make your first agent, and they will start helping.", style = Type.subheadline, color = Palette.secondary)
                    PrimaryButton("Make your first agent", action = onNewAgent)
                }
            }
        } else {
            item(key = "day") {
                DayStrip(
                    state,
                    now,
                    onLit,
                    { viewModel.open(HomePage.Routines) },
                    Modifier.padding(vertical = 8.dp).background(Palette.surface, RoundedRectangle(22.dp)).padding(horizontal = 16.dp, vertical = 12.dp),
                )
            }
            if (talks.main == null) {
                item(key = "checking") { Text(CHECKING_IN, Modifier.padding(vertical = 12.dp), style = Type.body, color = Palette.secondary) }
            } else {
                items(listOf(talks.main) + talks.talking + talks.quiet, key = { it.agent.id }) { AgentRow(it, state.roster, now, lit == it.agent.id, act) }
            }
        }
    }
}

/** One tab's content: shown and touchable only while selected; the other sits under it, deaf to touches. */
private fun Modifier.tabLayer(selected: Boolean): Modifier =
    zIndex(if (selected) 1f else 0f)
        .graphicsLayer { alpha = if (selected) 1f else 0f }
        .then(
            if (selected) {
                Modifier
            } else {
                Modifier.clearAndSetSemantics {}.pointerInput(Unit) {
                    awaitPointerEventScope { while (true) awaitPointerEvent(PointerEventPass.Initial).changes.forEach { it.consume() } }
                }
            },
        )

// Portions copyright 2026 Po Kai Lee and the Codync contributors.
// Licensed under the Apache License, Version 2.0; see licenses/Apache-2.0-Codync.txt.
// Adapted from Codync apps/android `MainTabs.kt` and `BotListView.kt`
// at ed8daf11f89398e598ef1f791abc3014ded68496.
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
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex
import com.kyant.shapes.Capsule
import kotlinx.coroutines.delay
import live.renoa.app.agents.AgentState
import live.renoa.app.badge.AgentHue
import live.renoa.app.badge.Badge
import live.renoa.app.theme.Appearance
import live.renoa.app.ui.LinkLine
import live.renoa.design.Edge
import live.renoa.design.GlassStage
import live.renoa.design.GlassTabBar
import live.renoa.design.Palette
import live.renoa.design.PrimaryButton
import live.renoa.design.PullToRefresh
import live.renoa.design.ScrollEdge
import live.renoa.design.Symbol
import live.renoa.design.TabItem
import live.renoa.design.ThinkingOrb
import live.renoa.design.ThinkingOrbState
import live.renoa.design.Type
import live.renoa.design.glass
import live.renoa.design.rememberPress
import live.renoa.design.semibold
import java.time.ZonedDateTime

/** Home's three tabs: the agents, Renoa's own pages, and this phone's settings. */
private enum class HomeTab { Agents, Renoa, Settings }

/**
 * Home is the owner's agents, Renoa's own pages (`Routines · Memory · Apps`), and Settings, under a
 * floating glass tab bar. Until it is [HomeUiState.known] who is here, Home waits quietly.
 */
@Composable
fun HomeScreen(
    viewModel: HomeViewModel,
    onOpenChat: (String) -> Unit,
    onNewAgent: () -> Unit,
    appearance: Appearance = Appearance.System,
    onAppearance: (Appearance) -> Unit = {},
    now: () -> ZonedDateTime = { ZonedDateTime.now() },
    onSignUp: () -> Unit = {},
    onSignOut: () -> Unit = {},
    onEditAgent: (String) -> Unit = {},
    onRefresh: () -> Unit = {},
    onNotificationsChanged: () -> Unit = {},
) {
    val state by viewModel.state.collectAsState()
    LaunchedEffect(state.signUp) { if (state.signUp) onSignUp() }
    val time by produceState(now()) {
        while (true) {
            delay(60_000L - value.second * 1_000L)
            value = now()
        }
    }
    var settingsOpen by rememberSaveable { mutableStateOf(false) }
    var accountOpen by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(state.page) { if (state.page != null) settingsOpen = false }
    val tab =
        when {
            settingsOpen -> HomeTab.Settings
            state.page != null -> HomeTab.Renoa
            else -> HomeTab.Agents
        }
    val act =
        TalkActions(
            open = { talk ->
                talk.note?.let { viewModel.seen(it.id) }
                onOpenChat(talk.agent.id)
            },
            answer = viewModel::answer,
            runAgain = viewModel::runAgain,
            edit = onEditAgent,
            markRead = viewModel::markRead,
        )
    BackHandler(enabled = tab != HomeTab.Agents) {
        settingsOpen = false
        viewModel.close()
    }

    val top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()
    val bottom = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()
    val barSpace = bottom + 62.dp + 24.dp
    GlassStage(
        Modifier.fillMaxSize().background(Palette.background),
        chrome = {
            Box(Modifier.align(Alignment.BottomCenter), contentAlignment = Alignment.BottomCenter) {
                ScrollEdge(Edge.Bottom, barSpace)
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    // A refusal stays in sight above the bar, however far the list scrolled.
                    state.notice?.let { Text(it, Modifier.padding(horizontal = 24.dp, vertical = 8.dp), style = Type.footnote, color = Palette.danger) }
                    if (state.known) {
                        GlassTabBar(
                            tab,
                            listOf(
                                TabItem(HomeTab.Agents, "Agents", "bubble.left.and.bubble.right.fill", 33.dp),
                                TabItem(HomeTab.Renoa, "Renoa", "hexagon.fill"),
                                TabItem(HomeTab.Settings, "Settings", "gearshape"),
                            ),
                            { picked ->
                                when (picked) {
                                    HomeTab.Agents -> {
                                        settingsOpen = false
                                        viewModel.close()
                                    }
                                    HomeTab.Renoa -> {
                                        settingsOpen = false
                                        // Tapping the Renoa tab while it shows keeps the page it is on.
                                        if (state.page == null) viewModel.open(HomePage.Routines)
                                    }
                                    HomeTab.Settings -> {
                                        viewModel.close()
                                        settingsOpen = true
                                    }
                                }
                            },
                            Modifier.padding(bottom = maxOf(bottom - 4.dp, 12.dp)),
                        )
                    }
                }
            }
        },
    ) {
        Box(Modifier.fillMaxSize().tabLayer(tab == HomeTab.Agents)) {
            AgentsRoster(state, time, act, { accountOpen = true }, onNewAgent, onRefresh, top, barSpace)
        }
        Box(Modifier.fillMaxSize().tabLayer(tab == HomeTab.Renoa)) {
            val renoa = remember(viewModel, onOpenChat) { RenoaActions(viewModel::open, viewModel::read, viewModel::switch, viewModel::switchApp, onOpenChat) }
            RenoaTab(state, time, state.page ?: HomePage.Routines, renoa, barSpace)
        }
        Box(Modifier.fillMaxSize().tabLayer(tab == HomeTab.Settings)) {
            SettingsPage(appearance, onAppearance, onSignOut, onNotificationsChanged, viewModel::approveComputer, barSpace)
        }
    }
    AccountSheet(accountOpen, { accountOpen = false }, state, onSignOut)
}

/** The Agents tab: floating glass account switcher and new-agent button over the flat roster. */
@Composable
private fun AgentsRoster(
    state: HomeUiState,
    now: ZonedDateTime,
    act: TalkActions,
    onOpenAccount: () -> Unit,
    onNewAgent: () -> Unit,
    onRefresh: () -> Unit,
    top: Dp,
    bottom: Dp,
) {
    GlassStage(
        Modifier.fillMaxSize().background(Palette.background),
        chrome = {
            ScrollEdge(Edge.Top, top + 64.dp)
            AgentsTopBar(state, onOpenAccount, onNewAgent)
        },
    ) {
        PullToRefresh({ onRefresh() }, Modifier.fillMaxSize(), indicatorTop = top + 50.dp) {
            val talks = state.talks
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = top + 56.dp, bottom = bottom),
            ) {
                when {
                    state.known && talks.main == null ->
                        item(key = "empty") { EmptyRoster(onNewAgent) }
                    talks.main == null ->
                        item(key = "checking") {
                            Row(
                                Modifier.fillMaxWidth().padding(vertical = 40.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(10.dp, Alignment.CenterHorizontally),
                            ) {
                                ThinkingOrb(state = ThinkingOrbState.Connecting, size = 20.dp, color = Palette.secondary)
                                Text(CHECKING_IN, style = Type.body, color = Palette.secondary)
                            }
                        }
                    else ->
                        items(listOf(talks.main) + talks.talking + talks.quiet, key = { it.agent.id }) { talk ->
                            AgentRow(talk, state.roster, now, act)
                        }
                }
            }
        }
    }
}

@Composable
private fun AgentsTopBar(state: HomeUiState, onOpenAccount: () -> Unit, onNewAgent: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().statusBarsPadding().padding(top = 6.dp).padding(horizontal = 16.dp).height(44.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val (source, pressed) = rememberPress()
        val who = state.ownerName ?: "Signed in"
        Row(
            Modifier
                .minimumInteractiveComponentSize()
                .height(44.dp)
                .glass(Capsule(), pressed = pressed)
                .clickable(source, null, role = Role.Button, onClick = onOpenAccount)
                .semantics {
                    contentDescription = "Account"
                    stateDescription = who
                }.padding(start = 7.dp, end = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Box(Modifier.size(30.dp).clip(Capsule()).background(Palette.bubbleUser), contentAlignment = Alignment.Center) {
                val initial = state.ownerName?.trim()?.take(1)?.uppercase()
                if (initial.isNullOrEmpty()) Symbol("person.fill", size = 16.dp, tint = Palette.tertiary)
                else Text(initial, style = Type.subheadline.semibold, color = Palette.text)
            }
            Symbol("chevron.down", size = 21.dp, tint = Palette.secondary)
        }
        Box(Modifier.weight(1f), contentAlignment = Alignment.Center) {
            LinkLine(state.link)
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

@Composable
private fun EmptyRoster(onNewAgent: () -> Unit) {
    Column(
        Modifier.fillMaxWidth().padding(vertical = 40.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Badge("Renoa", AgentState.Resting, 72.dp, primary = true, hue = AgentHue.Ink, label = "")
        Text("Nobody is here yet", style = Type.title3.semibold, color = Palette.text)
        Text(
            "Make your first agent, and they will start helping.",
            style = Type.subheadline,
            color = Palette.secondary,
            textAlign = TextAlign.Center,
        )
        PrimaryButton("Make your first agent", action = onNewAgent)
    }
}

/** One tab's content: shown and touchable only while selected; the others sit under it, deaf to touches. */
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

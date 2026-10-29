package live.renoa.app.home

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import live.renoa.app.agents.Routine
import live.renoa.app.badge.Badge
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaShapes
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.Glyphs
import live.renoa.app.ui.PathIcon
import java.time.ZonedDateTime

/** What tapping the dock does, as a screen reader says it. */
internal const val OPEN_PAGES = "Open chats, routines and what's coming up"

/** The bar at the bottom of Home: who wrote, and what runs next. It opens the pages. */
@Composable
internal fun Dock(state: HomeUiState, next: String, onOpen: () -> Unit, modifier: Modifier = Modifier) {
    val colors = Renoa.colors
    val fresh = freshFrom(state.roster)
    val shape = RoundedCornerShape(RenoaShapes.Card)
    Row(
        modifier
            .fillMaxWidth()
            .background(colors.card, shape)
            .border(1.dp, colors.border, shape)
            // Its words are what a screen reader says; the tap says where it goes.
            .clickable(role = Role.Button, onClickLabel = OPEN_PAGES, onClick = onOpen)
            .heightIn(min = 60.dp)
            .padding(horizontal = 14.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (fresh.isNotEmpty()) {
            Box(Modifier.width(24.dp + 14.dp * (fresh.size.coerceAtMost(3) - 1))) {
                fresh.take(3).forEachIndexed { i, agent ->
                    Badge(agent, state.roster, 24.dp, Modifier.offset(x = 14.dp * i), label = "")
                }
            }
            Spacer(Modifier.width(12.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(dockTitle(fresh), style = RenoaType.Label, color = colors.foreground, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(next, style = RenoaType.Caption, color = colors.mutedForeground, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        PathIcon(Glyphs.OPEN_UP, colors.foreground, size = 18.dp)
    }
}

/** The pages the dock opens: Chats, Routines and Upcoming, swiped through or picked from the tabs. */
@Composable
internal fun HomeDrawer(
    state: HomeUiState,
    now: ZonedDateTime,
    onPage: (HomePage) -> Unit,
    onClose: () -> Unit,
    onOpenChat: (String) -> Unit,
    onAnswer: (String, String) -> Unit,
    onNewAgent: () -> Unit,
    onSwitch: (String, Boolean) -> Unit,
) {
    val colors = Renoa.colors
    val reduced = LocalReducedMotion.current
    val pages = HomePage.entries
    val pager = rememberPagerState(initialPage = (state.page ?: HomePage.Chats).ordinal) { pages.size }
    val scope = rememberCoroutineScope()
    val open by rememberUpdatedState(state.page)
    val turn by rememberUpdatedState(onPage)
    // Opened again on another page while it was still closing, the pager turns to it.
    LaunchedEffect(state.page) { state.page?.let { if (pager.currentPage != it.ordinal) pager.scrollToPage(it.ordinal) } }
    // A page turned by swipe or tab is the page Home reopens on; once closed, a late turn reopens nothing.
    LaunchedEffect(pager) { snapshotFlow { pager.currentPage }.collect { if (open != null) turn(pages[it]) } }

    // A tap that lands on no control here stops here, not on Home underneath.
    Column(Modifier.fillMaxSize().background(colors.background).pointerInput(Unit) { detectTapGestures { } }.safeDrawingPadding()) {
        Row(Modifier.fillMaxWidth().padding(start = 12.dp, end = 4.dp, top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            pages.forEachIndexed { i, page ->
                val on = pager.currentPage == i
                val ink by animateColorAsState(if (on) colors.foreground else colors.mutedForeground, label = "tab")
                val underline by animateDpAsState(if (on) 2.dp else 0.dp, label = "tab line")
                Column(
                    Modifier
                        .heightIn(min = 48.dp)
                        .selectable(on, role = Role.Tab) { scope.launch { if (reduced) pager.scrollToPage(i) else pager.animateScrollToPage(i) } }
                        .padding(horizontal = 10.dp),
                    verticalArrangement = Arrangement.Center,
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Text(page.title, style = RenoaType.Label, color = ink)
                    Spacer(Modifier.height(6.dp))
                    Box(Modifier.width(18.dp).height(underline).background(colors.foreground, CircleShape))
                }
            }
            Spacer(Modifier.weight(1f))
            Box(
                Modifier.size(48.dp).clickable(role = Role.Button, onClick = onClose).semantics { contentDescription = "Back to Home" },
                contentAlignment = Alignment.Center,
            ) { PathIcon(Glyphs.CLOSE_DOWN, colors.foreground) }
        }
        HorizontalPager(pager, Modifier.weight(1f).fillMaxWidth()) { i ->
            when (pages[i]) {
                HomePage.Chats -> ChatsPage(state, onOpenChat, onAnswer, onNewAgent)
                HomePage.Routines -> RoutinesPage(state, now, onSwitch)
                HomePage.Upcoming -> UpcomingPage(state, now)
            }
        }
        state.notice?.let { Text(it, Modifier.padding(start = 24.dp, end = 24.dp, bottom = 8.dp), style = RenoaType.Caption, color = colors.stateFailed) }
    }
}

@Composable
private fun RoutinesPage(state: HomeUiState, now: ZonedDateTime, onSwitch: (String, Boolean) -> Unit) {
    val colors = Renoa.colors
    if (state.routines.isEmpty()) {
        Empty("No routines yet. Ask an agent to check on something every day, and it shows here.")
        return
    }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(vertical = 8.dp)) {
        items(state.routines, key = { it.id }) { routine ->
            val on = state.isOn(routine)
            Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 12.dp, top = 10.dp, bottom = 10.dp), horizontalArrangement = Arrangement.spacedBy(14.dp), verticalAlignment = Alignment.CenterVertically) {
                RoutineBadge(state, routine, 44.dp)
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text(routine.name, style = RenoaType.Label, color = colors.foreground)
                    Text(listOfNotNull(state.agent(routine.agentId)?.name, routine.schedule).joinToString(" · "), style = RenoaType.Caption, color = colors.foregroundSoft)
                    Text(routineStatus(routine.copy(on = on, next = routine.next.takeIf { on }), now), style = RenoaType.Caption, color = colors.mutedForeground)
                }
                RoutineSwitch(on, routine.name, enabled = routine.id !in state.switching) { onSwitch(routine.id, it) }
            }
        }
    }
}

@Composable
private fun UpcomingPage(state: HomeUiState, now: ZonedDateTime) {
    val colors = Renoa.colors
    val groups = upcoming(state.routines, now)
    if (groups.isEmpty()) {
        Empty("Nothing is coming up. The routines you turn on show here.")
        return
    }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(vertical = 8.dp)) {
        groups.forEach { group ->
            item(key = group.horizon.name) {
                Text(group.horizon.heading.uppercase(), Modifier.padding(start = 20.dp, top = 14.dp, bottom = 4.dp), style = RenoaType.Eyebrow, color = colors.mutedForeground)
            }
            items(group.runs, key = { "${group.horizon.name}/${it.first.id}" }) { (routine, time) ->
                Row(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 10.dp), horizontalArrangement = Arrangement.spacedBy(14.dp), verticalAlignment = Alignment.CenterVertically) {
                    RoutineBadge(state, routine, 36.dp)
                    Column(Modifier.weight(1f)) {
                        Text(routine.name, style = RenoaType.Label, color = colors.foreground)
                        state.agent(routine.agentId)?.let { Text(it.name, style = RenoaType.Caption, color = colors.foregroundSoft) }
                    }
                    Text(time, style = RenoaType.Caption, color = colors.mutedForeground)
                }
            }
        }
    }
}

/** The tile of the agent that runs [routine], or an empty socket while that agent is not on the roster. */
@Composable
private fun RoutineBadge(state: HomeUiState, routine: Routine, size: Dp) {
    val agent = state.agent(routine.agentId)
    if (agent == null) {
        EmptySocket(size, plus = false)
    } else {
        Badge(agent, state.roster, size, label = "")
    }
}

@Composable
private fun Empty(words: String) {
    Text(words, Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 32.dp), style = RenoaType.Body, color = Renoa.colors.mutedForeground)
}

/** An on/off switch in ink, tapped across 48dp. */
@Composable
private fun RoutineSwitch(on: Boolean, label: String, enabled: Boolean, onChange: (Boolean) -> Unit) {
    val colors = Renoa.colors
    val track by animateColorAsState(if (on) colors.foreground else colors.secondary, label = "switch")
    val knob by animateDpAsState(if (on) 18.dp else 2.dp, label = "knob")
    Box(
        Modifier
            .size(56.dp, 48.dp)
            .toggleable(on, enabled = enabled, role = Role.Switch, onValueChange = onChange)
            .semantics { contentDescription = label }
            .alpha(if (enabled) 1f else 0.45f),
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.size(40.dp, 24.dp).background(track, CircleShape).border(1.dp, if (on) colors.foreground else colors.input, CircleShape)) {
            Box(Modifier.offset { IntOffset(knob.roundToPx(), 2.dp.roundToPx()) }.size(20.dp).background(if (on) colors.primaryForeground else colors.mutedForeground, CircleShape))
        }
    }
}

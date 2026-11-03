package live.renoa.app.home

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
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
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.Velocity
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import live.renoa.app.agents.Routine
import live.renoa.app.badge.Badge
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.Glyphs
import live.renoa.app.ui.PathIcon
import java.time.ZonedDateTime

/**
 * The pages over Home: Chats, Routines and Upcoming, swiped through or picked from the tabs. Pulled
 * down by its top, or by a page already scrolled to its top, it follows the finger and closes once
 * pulled far enough or flung; short of that it springs back.
 */
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
    val pull = rememberPullToClose(onClose, reduced)
    LaunchedEffect(state.page) {
        state.page?.let {
            pull.reset()
            if (pager.currentPage != it.ordinal) pager.scrollToPage(it.ordinal)
        }
    }
    // A page turned by swipe or tab is the page Home reopens on; once closed, a late turn reopens nothing.
    LaunchedEffect(pager) { snapshotFlow { pager.currentPage }.collect { if (open != null) turn(pages[it]) } }

    // A tap that lands on no control here stops here, not on Home underneath.
    Column(
        Modifier
            .fillMaxSize()
            .graphicsLayer { translationY = pull.offset }
            .background(colors.background)
            .pointerInput(Unit) { detectTapGestures { } }
            .nestedScroll(pull)
            .safeDrawingPadding(),
    ) {
        Box(
            Modifier
                .fillMaxWidth()
                .draggable(rememberDraggableState { pull.drag(it) }, Orientation.Vertical, onDragStopped = { pull.release(it) })
                .padding(top = 8.dp),
            contentAlignment = Alignment.Center,
        ) { Box(Modifier.size(width = 36.dp, height = 4.dp).background(colors.border, CircleShape)) }
        Row(
            Modifier
                .fillMaxWidth()
                .draggable(rememberDraggableState { pull.drag(it) }, Orientation.Vertical, onDragStopped = { pull.release(it) })
                .padding(start = 12.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
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
    // A list of one, so a pull down on an empty page reaches the pages' pull to close.
    LazyColumn(Modifier.fillMaxSize()) {
        item { Text(words, Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 32.dp), style = RenoaType.Body, color = Renoa.colors.mutedForeground) }
    }
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

/**
 * How far the pages are pulled down, and whether letting go closes them: pulled past [far], or
 * flung down faster than [fast] once past [slop]. Short of that they spring back, at once when
 * motion is reduced; a new pull takes over from a spring-back still running.
 */
private class PullToClose(
    private val close: () -> Unit,
    private val scope: CoroutineScope,
    private val reduced: Boolean,
    private val far: Float,
    private val fast: Float,
    private val slop: Float,
) : NestedScrollConnection {
    var offset by mutableFloatStateOf(0f)
        private set
    private var back: Job? = null

    fun drag(dy: Float) {
        back?.cancel()
        offset = (offset + dy).coerceAtLeast(0f)
    }

    fun reset() {
        back?.cancel()
        offset = 0f
    }

    fun release(velocity: Float) {
        if (offset <= 0f) return
        if (offset > far || (offset > slop && velocity > fast)) {
            close()
        } else if (reduced) {
            offset = 0f
        } else {
            back = scope.launch { animate(offset, 0f) { value, _ -> offset = value } }
        }
    }

    // While pulled, a drag up takes the pull back before the page scrolls.
    override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset {
        if (offset <= 0f || available.y >= 0f || source != NestedScrollSource.UserInput) return Offset.Zero
        val used = maxOf(available.y, -offset)
        drag(used)
        return Offset(0f, used)
    }

    // A drag down that the page, at its top, leaves over pulls the pages.
    override fun onPostScroll(consumed: Offset, available: Offset, source: NestedScrollSource): Offset {
        if (available.y <= 0f || source != NestedScrollSource.UserInput) return Offset.Zero
        drag(available.y)
        return Offset(0f, available.y)
    }

    override suspend fun onPreFling(available: Velocity): Velocity {
        if (offset <= 0f) return Velocity.Zero
        release(available.y)
        return available
    }
}

/** The pages close pulled 120dp, a third of a 360dp phone, or flung faster than 1,200dp a second once pulled 24dp. */
@Composable
private fun rememberPullToClose(onClose: () -> Unit, reduced: Boolean): PullToClose {
    val close by rememberUpdatedState(onClose)
    val density = LocalDensity.current
    val scope = rememberCoroutineScope()
    return remember(density, scope, reduced) {
        with(density) { PullToClose({ close() }, scope, reduced, far = 120.dp.toPx(), fast = 1_200.dp.toPx(), slop = 24.dp.toPx()) }
    }
}

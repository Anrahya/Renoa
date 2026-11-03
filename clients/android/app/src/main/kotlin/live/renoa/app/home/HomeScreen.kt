package live.renoa.app.home

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Text
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
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.ButtonKind
import live.renoa.app.ui.ButtonSize
import live.renoa.app.ui.LinkLine
import live.renoa.app.ui.RenoaButton
import live.renoa.app.ui.RenoaMark
import java.time.ZonedDateTime

/**
 * Home is the owner's day with their agents: the day strip on top, then the main agent with what
 * needs the owner, then every agent with something to say, and the quiet ones in a line each.
 * Until it is [HomeUiState.known] who is here, Home says it is checking in.
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
    val colors = Renoa.colors
    var lit by remember { mutableStateOf<String?>(null) }
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
            Text(greeting(time.hour, state.ownerName), Modifier.padding(horizontal = 20.dp), style = RenoaType.Title, color = colors.foreground)
            LinkLine(state.link, Modifier.padding(start = 20.dp, end = 20.dp, top = 6.dp))
            DayStrip(state, time, { lit = it }, Modifier.padding(start = 20.dp, end = 20.dp, top = 10.dp))
            TalkList(state, time, lit, act, onNewAgent, Modifier.weight(1f))
            state.notice?.takeIf { state.page == null }?.let { Text(it, Modifier.padding(start = 24.dp, top = 6.dp), style = RenoaType.Caption, color = colors.stateFailed) }
            if (state.known) {
                Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 8.dp, bottom = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    RenoaButton(HomePage.Routines.title, { viewModel.open(HomePage.Routines) }, Modifier.weight(1f), kind = ButtonKind.Secondary)
                    RenoaButton("New helper", onNewAgent, Modifier.weight(1f), kind = ButtonKind.Secondary)
                }
            }
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

/** The main agent pinned on top, everyone with something to say, then the quiet ones. */
@Composable
private fun TalkList(state: HomeUiState, now: ZonedDateTime, lit: String?, act: TalkActions, onNewAgent: () -> Unit, modifier: Modifier = Modifier) {
    val colors = Renoa.colors
    val talks = state.talks
    LazyColumn(modifier.fillMaxWidth(), contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 16.dp)) {
        when {
            !state.known -> item(key = "checking") { Text(CHECKING_IN, Modifier.padding(4.dp), style = RenoaType.Body, color = colors.mutedForeground) }
            talks.main == null ->
                item(key = "first") {
                    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                        Text("Nobody is here yet.", style = RenoaType.Heading, color = colors.foreground)
                        Text("Make your first agent, and they will start helping.", style = RenoaType.Body, color = colors.foregroundSoft)
                        RenoaButton("Make your first agent", onNewAgent, size = ButtonSize.Sm)
                    }
                }
            else -> {
                val main = talks.main
                item(key = main.agent.id) { MainCard(main, state.roster, nextWords(state.routines, now), now, lit == main.agent.id, act, Modifier.padding(bottom = 6.dp)) }
                items(talks.talking, key = { it.agent.id }) { TalkRow(it, state.roster, now, lit == it.agent.id, act) }
                if (talks.quiet.isNotEmpty()) {
                    item(key = "quiet") { Text("QUIET", Modifier.padding(start = 4.dp, top = 16.dp, bottom = 4.dp), style = RenoaType.Eyebrow, color = colors.mutedForeground) }
                    items(talks.quiet, key = { it.agent.id }) { QuietRow(it, state.roster, lit == it.agent.id, act) }
                }
            }
        }
    }
}

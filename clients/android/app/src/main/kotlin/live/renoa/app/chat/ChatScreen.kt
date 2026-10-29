package live.renoa.app.chat

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import live.renoa.app.agents.ChatMessage
import live.renoa.app.agents.Speaker
import live.renoa.app.badge.Badge
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaEasing
import live.renoa.app.theme.RenoaShapes
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.LinkLine
import live.renoa.app.ui.AskCard
import live.renoa.app.ui.Glyphs
import live.renoa.app.ui.PathIcon

@Composable
fun ChatScreen(viewModel: ChatViewModel, onBack: () -> Unit, onOpenProfile: (String) -> Unit) {
    val state by viewModel.state.collectAsState()
    val colors = Renoa.colors
    val agent = state.agent
    val list = rememberLazyListState()
    val focus = LocalFocusManager.current
    val keyboard = LocalSoftwareKeyboardController.current
    // Sending, from the button or the keyboard, puts the keyboard away so the conversation is in view; the list goes to the message once it shows.
    val send = {
        if (state.action == ComposerAction.Send) {
            viewModel.send()
            keyboard?.hide()
            focus.clearFocus()
        }
    }
    // Messages already here when the chat first loads show whole; only later ones rise, and only once.
    val old = remember(state.loaded) { if (state.loaded) (state.messages.map { it.id } + listOfNotNull(state.streaming?.id)).toHashSet() else emptySet() }
    val risen = remember { mutableSetOf<String>() }
    // Whatever arrives while the chat is open has been read.
    LaunchedEffect(state.messages.size) { viewModel.seen() }
    QuickWifi()

    Box(Modifier.fillMaxSize().background(colors.background)) {
        Column(Modifier.fillMaxSize().safeDrawingPadding()) {
            Row(Modifier.fillMaxWidth().padding(start = 4.dp, end = 8.dp, top = 10.dp, bottom = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(48.dp).clip(CircleShape).clickable(role = Role.Button, onClickLabel = "Back", onClick = onBack), contentAlignment = Alignment.Center) {
                    PathIcon(Glyphs.BACK, colors.foreground)
                }
                Row(
                    Modifier.weight(1f).clickable(enabled = agent != null) { agent?.let { onOpenProfile(it.id) } }.padding(start = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    if (agent != null) Badge(agent.name, agent.state, 50.dp, primary = state.main, arriveAfterMillis = 0, hue = state.hue)
                    Text(agent?.name.orEmpty(), style = RenoaType.Heading, color = colors.foreground)
                }
            }
            if (!state.loaded) {
                Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                    Text(OPENING, style = RenoaType.Caption, color = colors.mutedForeground)
                }
            } else if (state.empty) {
                Box(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()), contentAlignment = Alignment.Center) {
                    ChatStart(state, onStarter = viewModel::onDraft, Modifier.padding(vertical = 24.dp))
                }
            } else {
                Messages(state, list, old, risen, onRetry = viewModel::retry, onDiscard = viewModel::discard, Modifier.weight(1f))
            }
            state.ask?.let { AskCard(it, onAnswer = viewModel::answer, Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) }
            Composer(state, agent?.name.orEmpty(), viewModel, send)
        }
    }
}

/** The newest item, at the bottom: an animated scroll there, or a jump with reduced motion. */
private suspend fun LazyListState.toNewest(reduced: Boolean) = if (reduced) scrollToItem(0) else animateScrollToItem(0)

/** What the chat says until it has been read, as while Renoa cannot be reached. */
internal const val OPENING = "Connecting to Renoa…"

/**
 * The conversation, newest at the bottom. What the owner just said is brought into view, and so is
 * whatever arrives while they are at the bottom; while they have scrolled up to read, it waits below.
 */
@Composable
private fun Messages(
    state: ChatUiState,
    list: LazyListState,
    old: Set<String>,
    risen: MutableSet<String>,
    onRetry: (String) -> Unit,
    onDiscard: (String) -> Unit,
    modifier: Modifier,
) {
    val colors = Renoa.colors
    val agent = state.agent
    val reduced = LocalReducedMotion.current
    val showsWorking = agent != null && state.working && state.streaming == null
    // A reply still arriving is the same item as the message it becomes, so nothing jumps when it is whole.
    val streaming = state.streaming?.let { ChatMessage(it.id, Speaker.Agent, it.text) }
    val shown = if (streaming != null) state.messages + streaming else state.messages
    val newest = if (showsWorking) WORKING else shown.lastOrNull()?.id
    val atBottom by remember { derivedStateOf { list.firstVisibleItemIndex == 0 && list.firstVisibleItemScrollOffset == 0 } }
    // Read as this item arrives, before the list lays it out: whether the owner was at the bottom until now, or just spoke.
    val follow = atBottom || shown.lastOrNull()?.let { it.speaker == Speaker.Owner && it.id !in old } == true
    // Keyed on the last message too: what the owner says while the agent works sits under the working line.
    LaunchedEffect(newest, shown.lastOrNull()?.id) { if (follow) list.toNewest(reduced) }
    LazyColumn(
        modifier.fillMaxWidth(),
        state = list,
        // Room under the newest message, so it does not sit on the composer, and a little over the oldest.
        contentPadding = PaddingValues(top = 8.dp, bottom = 16.dp),
        reverseLayout = true,
        verticalArrangement = Arrangement.spacedBy(18.dp, Alignment.Top),
    ) {
        // Reversed layout: the first item sits at the bottom, nearest the composer.
        if (showsWorking) {
            item(key = WORKING) {
                Row(Modifier.padding(horizontal = 20.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Badge(agent.name, agent.state, 56.dp, primary = state.main, hue = state.hue)
                    WorkingLine(colors.foreground, agent.glance.doing, colors.mutedForeground)
                }
            }
        }
        items(shown.asReversed(), key = { it.id }) { message ->
            val fresh = message.id !in old && message.id !in risen
            SideEffect { risen += message.id }
            Rise(fresh) { MessageRow(message, complete = message.id != state.streaming?.id, animate = fresh, onRetry, onDiscard) }
        }
    }
}

@Composable
private fun Rise(animate: Boolean, content: @Composable () -> Unit) {
    val reduced = LocalReducedMotion.current
    val progress = remember { Animatable(if (animate && !reduced) 0f else 1f) }
    LaunchedEffect(Unit) { if (progress.value < 1f) progress.animateTo(1f, tween(800, easing = RenoaEasing.Ease)) }
    Box(Modifier.graphicsLayer { alpha = progress.value; translationY = (1f - progress.value) * 14.dp.toPx() }) { content() }
}

/** The key of the line that says the agent is working, the list's newest item while it shows. */
private const val WORKING = "working"

@Composable
private fun MessageRow(message: ChatMessage, complete: Boolean, animate: Boolean, onRetry: (String) -> Unit, onDiscard: (String) -> Unit) {
    val colors = Renoa.colors
    when (message.speaker) {
        Speaker.Owner ->
            Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp), horizontalAlignment = Alignment.End) {
                // A message that did not go is the retry button, with its line under it.
                Column(
                    if (message.notSent) Modifier.clickable(role = Role.Button, onClickLabel = "Retry") { onRetry(message.id) } else Modifier,
                    horizontalAlignment = Alignment.End,
                    verticalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Text(
                        message.text,
                        Modifier
                            .widthIn(max = 300.dp)
                            .background(colors.secondary, RoundedCornerShape(RenoaShapes.Bubble, RenoaShapes.Bubble, 6.dp, RenoaShapes.Bubble))
                            .padding(horizontal = 16.dp, vertical = 12.dp),
                        style = RenoaType.Body,
                        color = colors.foreground,
                    )
                    if (message.notSent) Text("Didn't send · Tap to retry", style = RenoaType.Caption, color = colors.stateFailed)
                }
                if (message.notSent) {
                    Box(
                        Modifier.heightIn(min = 48.dp).clickable(role = Role.Button) { onDiscard(message.id) }.padding(horizontal = 4.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text("Delete", style = RenoaType.Caption, color = colors.mutedForeground)
                    }
                }
            }
        Speaker.Agent ->
            RevealText(message.text, complete, animate, RenoaType.Body, colors.foregroundSoft, Modifier.padding(horizontal = 20.dp).fillMaxWidth(0.94f))
    }
}

@Composable
private fun Composer(state: ChatUiState, name: String, viewModel: ChatViewModel, send: () -> Unit) {
    val colors = Renoa.colors
    Column(Modifier.padding(start = 16.dp, end = 16.dp, top = 6.dp, bottom = 12.dp)) {
        LinkLine(state.link, Modifier.padding(start = 8.dp, bottom = 6.dp))
        state.notice?.let {
            Text(it, Modifier.padding(start = 8.dp, bottom = 6.dp), style = RenoaType.Caption, color = colors.stateFailed)
        }
        Row(
            Modifier
                .fillMaxWidth()
                .background(colors.card, RoundedCornerShape(RenoaShapes.Composer))
                .border(1.dp, colors.input, RoundedCornerShape(RenoaShapes.Composer))
                .padding(start = 16.dp, end = 6.dp, top = 6.dp, bottom = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Box(Modifier.weight(1f).padding(vertical = 10.dp)) {
                if (state.draft.isEmpty()) Text("Message $name".trim(), style = RenoaType.Body, color = colors.mutedForeground)
                BasicTextField(
                    value = state.draft,
                    onValueChange = viewModel::onDraft,
                    textStyle = RenoaType.Body.copy(color = colors.foreground),
                    cursorBrush = SolidColor(colors.ring),
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Send),
                    keyboardActions = KeyboardActions(onSend = { send() }),
                    modifier = Modifier.fillMaxWidth().semantics { contentDescription = "Message $name".trim() },
                )
            }
            val enabled = state.action != ComposerAction.Idle
            Box(
                Modifier
                    .size(40.dp)
                    .alpha(if (enabled) 1f else 0.45f)
                    .background(colors.primary, RoundedCornerShape(RenoaShapes.Composer - 6.dp))
                    .semantics { contentDescription = if (state.action == ComposerAction.Stop) "Stop" else "Send" }
                    .clickable(enabled = enabled, role = Role.Button) {
                        if (state.action == ComposerAction.Stop) viewModel.stop() else send()
                    },
                contentAlignment = Alignment.Center,
            ) {
                if (state.action == ComposerAction.Stop) {
                    PathIcon(Glyphs.STOP, colors.primaryForeground, size = 16.dp)
                } else {
                    PathIcon(Glyphs.SEND, colors.primaryForeground, size = 18.dp)
                }
            }
        }
    }
}

// Portions copyright 2026 Po Kai Lee and the Codync contributors.
// Licensed under the Apache License, Version 2.0; see licenses/Apache-2.0-Codync.txt.
// The header, bubbles and composer are adapted from Codync apps/android `ChatScreen.kt`, `ChatRows.kt`
// and `MessageComposer.kt` at ed8daf11f89398e598ef1f791abc3014ded68496.
package live.renoa.app.chat

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.togetherWith
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
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.kyant.shapes.Capsule
import com.kyant.shapes.RoundedRectangle
import live.renoa.app.agents.ChatMessage
import live.renoa.app.agents.Speaker
import live.renoa.app.badge.Badge
import live.renoa.app.badge.describe
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.RenoaShapes
import live.renoa.app.ui.AskCard
import live.renoa.app.ui.LinkLine
import live.renoa.design.Edge
import live.renoa.design.GlassStage
import live.renoa.design.Motion
import live.renoa.design.Palette
import live.renoa.design.ScrollEdge
import live.renoa.design.Symbol
import live.renoa.design.Type
import live.renoa.design.bold
import live.renoa.design.glass
import live.renoa.design.plainButton
import live.renoa.design.pressScale
import live.renoa.design.rememberPress
import live.renoa.design.semibold
import live.renoa.rcp.Attachment

@Composable
fun ChatScreen(viewModel: ChatViewModel, onBack: () -> Unit, onOpenProfile: (String) -> Unit) {
    val state by viewModel.state.collectAsState()
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
    val parses = remember { ParsedReplies() }
    // Whatever arrives while the chat is open has been read.
    LaunchedEffect(state.messages.size) { viewModel.seen() }
    QuickWifi()

    val top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()
    val density = LocalDensity.current
    var composerHeight by remember { mutableIntStateOf(0) }
    val under = with(density) { composerHeight.toDp() }
    GlassStage(
        Modifier.fillMaxSize().background(Palette.background),
        chrome = {
            ScrollEdge(Edge.Top, top + 84.dp)
            ScrollEdge(Edge.Bottom, under + 4.dp, Modifier.align(Alignment.BottomCenter))
            Header(state, onBack, onOpenProfile)
            // Measured with the keyboard and navigation bar under it, so the last message clears them too.
            Column(Modifier.align(Alignment.BottomCenter).fillMaxWidth().onSizeChanged { composerHeight = it.height }.imePadding().navigationBarsPadding()) {
                LinkLine(state.link, Modifier.padding(start = 24.dp, bottom = 6.dp))
                state.notice?.let { Text(it, Modifier.padding(start = 24.dp, end = 24.dp, bottom = 6.dp), style = Type.footnote, color = Palette.danger) }
                state.ask?.let { AskCard(it, onAnswer = viewModel::answer, Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) }
                Composer(state, agent?.name.orEmpty(), viewModel, send)
            }
        },
    ) {
        val padding = PaddingValues(top = top + 60.dp, bottom = under + 8.dp)
        // Unseen until the composer is measured, so the conversation never shows a frame too low.
        Box(Modifier.fillMaxSize().graphicsLayer { alpha = if (composerHeight > 0) 1f else 0f }) {
        when {
            !state.loaded ->
                Box(Modifier.fillMaxSize().padding(padding), contentAlignment = Alignment.Center) { Text(OPENING, style = Type.footnote, color = Palette.secondary) }
            state.empty ->
                Box(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(padding), contentAlignment = Alignment.Center) {
                    ChatStart(state, onStarter = viewModel::onDraft, Modifier.padding(vertical = 24.dp))
                }
            else -> Messages(state, list, old, risen, parses, onRetry = viewModel::retry, onDiscard = viewModel::discard, padding)
        }
        }
    }
}

/** The glass back button, and the agent with its state under its name, which opens its profile. */
@Composable
private fun Header(state: ChatUiState, onBack: () -> Unit, onOpenProfile: (String) -> Unit) {
    val agent = state.agent
    Box(Modifier.fillMaxWidth().statusBarsPadding().padding(top = 6.dp).padding(horizontal = 16.dp).height(44.dp)) {
        val (source, pressed) = rememberPress()
        Box(
            Modifier
                .align(Alignment.CenterStart)
                .minimumInteractiveComponentSize()
                .size(44.dp)
                .glass(Capsule(), pressed = pressed)
                .clickable(source, null, role = Role.Button, onClick = onBack)
                .semantics { contentDescription = "Back" },
            contentAlignment = Alignment.Center,
        ) { Symbol("chevron.left", size = 32.dp, tint = Palette.text) }
        if (agent != null) {
            Column(
                Modifier.align(Alignment.Center).padding(horizontal = 56.dp).plainButton { onOpenProfile(agent.id) },
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(7.dp)) {
                    Badge(agent.name, agent.state, 26.dp, primary = state.main, arriveAfterMillis = 0, hue = state.hue)
                    Text(agent.name, style = Type.body.semibold, color = Palette.text, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                Text(if (state.working) "Working" else agent.state.describe().replaceFirstChar { it.uppercase() }, style = Type.caption, color = Palette.secondary, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
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
    parses: ParsedReplies,
    onRetry: (String) -> Unit,
    onDiscard: (String) -> Unit,
    padding: PaddingValues,
) {
    val agent = state.agent
    val reduced = LocalReducedMotion.current
    val showsWorking = agent != null && state.working && state.streaming == null
    // A reply still arriving is the same item as the message it becomes, so nothing jumps when it is whole.
    // Once that message is here, it alone shows, even before the reply in progress is cleared.
    val arriving = state.streaming?.takeIf { reply -> state.messages.none { it.id == reply.id } }?.let { ChatMessage(it.id, Speaker.Agent, it.text) }
    val shown = if (arriving != null) state.messages + arriving else state.messages
    val newest = if (showsWorking) WORKING else shown.lastOrNull()?.id
    val atBottom by remember { derivedStateOf { list.firstVisibleItemIndex == 0 && list.firstVisibleItemScrollOffset == 0 } }
    // Read as this item arrives, before the list lays it out: whether the owner was at the bottom until now, or just spoke.
    val follow = atBottom || shown.lastOrNull()?.let { it.speaker == Speaker.Owner && it.id !in old } == true
    // Keyed on the last message too: what the owner says while the agent works sits under the working line.
    LaunchedEffect(newest, shown.lastOrNull()?.id) { if (follow) list.toNewest(reduced) }
    LazyColumn(
        Modifier.fillMaxSize(),
        state = list,
        // Under the header and over the composer, which float over the conversation.
        contentPadding = padding,
        reverseLayout = true,
        verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.Top),
    ) {
        // Reversed layout: the first item sits at the bottom, nearest the composer.
        if (showsWorking) {
            item(key = WORKING) {
                Row(
                    Modifier.padding(start = 16.dp, end = 56.dp).clip(RoundedRectangle(RenoaShapes.Bubble)).background(Palette.bubbleAgent).padding(horizontal = 14.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Badge(agent.name, agent.state, 30.dp, primary = state.main, hue = state.hue)
                    WorkingLine(Palette.text, agent.glance.doing, Palette.secondary)
                }
            }
        }
        items(shown.asReversed(), key = { it.id }) { message ->
            val fresh = message.id !in old && message.id !in risen
            SideEffect { risen += message.id }
            Rise(fresh) { MessageRow(message, streaming = message.id == arriving?.id, parses, onRetry, onDiscard) }
        }
    }
}

@Composable
private fun Rise(animate: Boolean, content: @Composable () -> Unit) {
    val reduced = LocalReducedMotion.current
    val progress = remember { Animatable(if (animate && !reduced) 0f else 1f) }
    LaunchedEffect(Unit) { if (progress.value < 1f) progress.animateTo(1f, Motion.conversation()) }
    Box(Modifier.graphicsLayer { alpha = progress.value; translationY = (1f - progress.value) * 14.dp.toPx() }) { content() }
}

/** The key of the line that says the agent is working, the list's newest item while it shows. */
private const val WORKING = "working"

@Composable
private fun MessageRow(message: ChatMessage, streaming: Boolean, parses: ParsedReplies, onRetry: (String) -> Unit, onDiscard: (String) -> Unit) {
    when (message.speaker) {
        Speaker.Owner ->
            Column(Modifier.fillMaxWidth().padding(start = 56.dp, end = 16.dp), horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(4.dp)) {
                // A message that did not go is the retry button, with its line under it.
                Column(
                    if (message.notSent) Modifier.clickable(role = Role.Button, onClickLabel = "Retry") { onRetry(message.id) } else Modifier,
                    horizontalAlignment = Alignment.End,
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    if (message.attachments.isNotEmpty()) SentFiles(message.attachments)
                    if (message.text.isNotBlank()) {
                        Text(
                            message.text,
                            Modifier.widthIn(max = 300.dp).clip(RoundedRectangle(RenoaShapes.Bubble)).background(Palette.bubbleUser).padding(horizontal = 16.dp, vertical = 10.dp),
                            style = Type.body,
                            color = Palette.text,
                        )
                    }
                    if (message.notSent) Text("Didn't send · Tap to retry", style = Type.caption2.bold, color = Palette.danger)
                }
                if (message.notSent) {
                    Box(
                        Modifier.heightIn(min = 48.dp).clickable(role = Role.Button) { onDiscard(message.id) }.padding(horizontal = 4.dp),
                        contentAlignment = Alignment.Center,
                    ) { Text("Delete", style = Type.footnote, color = Palette.secondary) }
                }
            }
        Speaker.Event ->
            Text(
                message.text,
                Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 2.dp),
                style = Type.footnote,
                color = Palette.tertiary,
                textAlign = TextAlign.Center,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        // The owner's own words stay plain; an agent's reply is markdown, in its bubble.
        Speaker.Agent -> {
            val modifier = Modifier.padding(start = 16.dp, end = 40.dp).clip(RoundedRectangle(RenoaShapes.Bubble)).background(Palette.bubbleAgent).padding(horizontal = 16.dp, vertical = 10.dp)
            // One for both, so a diagram drawn while the reply arrives stays drawn once the reply is whole.
            val diagrams = remember { ReplyDiagrams() }
            if (streaming) ArrivingReply(message.text, modifier, diagrams) else Reply(message.text, parses.of(message.id, message.text), modifier, diagrams)
        }
    }
}

/** The files waiting to go, then the field in glass with the "+" and one button that turns between send and stop. */
@Composable
private fun Composer(state: ChatUiState, name: String, viewModel: ChatViewModel, send: () -> Unit) {
    if (state.files.isNotEmpty()) PickedFiles(state.files, viewModel::unattach, Modifier.padding(start = 16.dp, end = 16.dp, top = 6.dp))
    Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 6.dp, bottom = 16.dp), verticalAlignment = Alignment.Bottom) {
        Row(
            Modifier.weight(1f).glass(RoundedRectangle(RenoaShapes.Composer)).animateContentSize(Motion.layoutSize).padding(vertical = 6.dp),
            verticalAlignment = Alignment.Bottom,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            AttachButton(Attachment.MAX_PER_COMMAND - state.files.size, viewModel::attach, viewModel::problem)
            BasicTextField(
                value = state.draft,
                onValueChange = viewModel::onDraft,
                textStyle = Type.body.copy(color = Palette.text),
                cursorBrush = SolidColor(Palette.accent),
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Send),
                keyboardActions = KeyboardActions(onSend = { send() }),
                maxLines = 8,
                modifier = Modifier.weight(1f).padding(vertical = 10.dp).semantics { contentDescription = "Message $name".trim() },
                decorationBox = { input ->
                    Box {
                        if (state.draft.isEmpty()) Text("Message $name".trim(), style = Type.body, color = Palette.tertiary, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        input()
                    }
                },
            )
            SendButton(state.action, Modifier) { if (state.action == ComposerAction.Stop) viewModel.stop() else send() }
        }
    }
}

/** Send or stop: its symbol swaps, its fill crossfades, and it dims while there is nothing to send. */
@Composable
private fun SendButton(action: ComposerAction, modifier: Modifier, onClick: () -> Unit) {
    val enabled = action != ComposerAction.Idle
    val fill by animateColorAsState(if (enabled) Palette.accentFill else Palette.accentDim, Motion.morph(), label = "Fill")
    val ink by animateColorAsState(if (enabled) Palette.onAccent else Palette.tertiary, Motion.morph(), label = "Ink")
    val (source, pressed) = rememberPress()
    // The tap area is the 48dp round the 34dp capsule.
    Box(
        modifier
            .minimumInteractiveComponentSize()
            .clickable(source, null, enabled = enabled, role = Role.Button, onClick = onClick)
            .semantics { contentDescription = if (action == ComposerAction.Stop) "Stop" else "Send" }
            .pressScale(pressed)
            .size(34.dp)
            .clip(Capsule())
            .background(fill),
        contentAlignment = Alignment.Center,
    ) {
        AnimatedContent(
            action == ComposerAction.Stop,
            transitionSpec = { (fadeIn(Motion.morph()) + scaleIn(Motion.morph(), 0.6f)) togetherWith (fadeOut(Motion.morph()) + scaleOut(Motion.morph(), 0.6f)) },
            label = "Symbol",
        ) { stop -> if (stop) Symbol("stop.fill", size = 16.dp, tint = ink) else Symbol("arrow.up", size = 19.dp, tint = ink) }
    }
}

package live.renoa.app.chat

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import live.renoa.app.agents.ChatMessage
import live.renoa.app.agents.Speaker
import live.renoa.app.badge.AgentLook
import live.renoa.app.badge.Badge
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaEasing
import live.renoa.app.theme.RenoaShapes
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.AskCard
import live.renoa.app.ui.Glyphs
import live.renoa.app.ui.PathIcon

@Composable
fun ChatScreen(viewModel: ChatViewModel, onBack: () -> Unit, onOpenProfile: (String) -> Unit) {
    val state by viewModel.state.collectAsState()
    val colors = Renoa.colors
    val agent = state.agent
    val look = remember(agent?.name) { AgentLook.forName(agent?.name ?: "?") }
    // Messages already here when the chat first loads show whole; only later ones rise, and only once.
    val old = remember(state.loaded) { if (state.loaded) (state.messages.map { it.id } + listOfNotNull(state.streaming?.id)).toHashSet() else emptySet() }
    val risen = remember { mutableSetOf<String>() }
    // Whatever arrives while the chat is open has been read.
    LaunchedEffect(state.messages.size) { viewModel.seen() }

    Box(Modifier.fillMaxSize().background(colors.background)) {
        Column(Modifier.fillMaxSize().safeDrawingPadding()) {
            Row(Modifier.fillMaxWidth().padding(start = 4.dp, end = 8.dp, top = 10.dp, bottom = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(44.dp).clip(CircleShape).clickable(role = Role.Button, onClickLabel = "Back", onClick = onBack), contentAlignment = Alignment.Center) {
                    PathIcon(Glyphs.BACK, colors.foreground)
                }
                Row(
                    Modifier.weight(1f).clickable(enabled = agent != null) { agent?.let { onOpenProfile(it.id) } }.padding(start = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    if (agent != null) Badge(look, agent.state, 50.dp, label = agent.name)
                    Text(agent?.name.orEmpty(), style = RenoaType.Heading, color = colors.foreground)
                }
            }
            Messages(state, old, risen, Modifier.weight(1f), look)
            state.ask?.let { AskCard(it, onAnswer = viewModel::answer, Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) }
            Composer(state, agent?.name.orEmpty(), viewModel)
        }
    }
}

@Composable
private fun Messages(state: ChatUiState, old: Set<String>, risen: MutableSet<String>, modifier: Modifier, look: AgentLook) {
    val colors = Renoa.colors
    val agent = state.agent
    LazyColumn(
        modifier.fillMaxWidth(),
        reverseLayout = true,
        verticalArrangement = Arrangement.spacedBy(18.dp, Alignment.Top),
    ) {
        // Reversed layout: the first item sits at the bottom, nearest the composer.
        if (agent != null && state.working && state.streaming == null) {
            item(key = "working") {
                Row(Modifier.padding(horizontal = 20.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Badge(look, agent.state, 56.dp, label = agent.name)
                    WorkingLine(colors.foreground, agent.glance.doing, colors.mutedForeground)
                }
            }
        }
        // A reply still arriving is the same item as the message it becomes, so nothing jumps when it is whole.
        val streaming = state.streaming?.let { ChatMessage(it.id, Speaker.Agent, it.text) }
        val shown = if (streaming != null) state.messages + streaming else state.messages
        items(shown.asReversed(), key = { it.id }) { message ->
            val fresh = message.id !in old && message.id !in risen
            SideEffect { risen += message.id }
            Rise(fresh) { MessageRow(message, complete = message.id != state.streaming?.id, animate = fresh) }
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

@Composable
private fun MessageRow(message: ChatMessage, complete: Boolean, animate: Boolean) {
    val colors = Renoa.colors
    when (message.speaker) {
        Speaker.Owner ->
            Box(Modifier.fillMaxWidth().padding(horizontal = 20.dp), contentAlignment = Alignment.CenterEnd) {
                Text(
                    message.text,
                    Modifier
                        .widthIn(max = 300.dp)
                        .background(colors.secondary, RoundedCornerShape(RenoaShapes.Bubble, RenoaShapes.Bubble, 6.dp, RenoaShapes.Bubble))
                        .padding(horizontal = 16.dp, vertical = 12.dp),
                    style = RenoaType.Body,
                    color = colors.foreground,
                )
            }
        Speaker.Agent ->
            RevealText(message.text, complete, animate, RenoaType.Body, colors.foregroundSoft, Modifier.padding(horizontal = 20.dp).fillMaxWidth(0.94f))
    }
}

@Composable
private fun Composer(state: ChatUiState, name: String, viewModel: ChatViewModel) {
    val colors = Renoa.colors
    Column(Modifier.padding(start = 16.dp, end = 16.dp, top = 6.dp, bottom = 12.dp)) {
        state.notice?.let {
            Text(it, Modifier.padding(start = 8.dp, bottom = 6.dp), style = RenoaType.Caption, color = colors.stateFailed)
        }
        Row(
            Modifier
                .fillMaxWidth()
                .background(colors.card, RoundedCornerShape(RenoaShapes.Control))
                .border(1.dp, colors.input, RoundedCornerShape(RenoaShapes.Control))
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
                    modifier = Modifier.fillMaxWidth().semantics { contentDescription = "Message $name".trim() },
                )
            }
            val enabled = state.action != ComposerAction.Idle
            Box(
                Modifier
                    .size(40.dp)
                    .alpha(if (enabled) 1f else 0.45f)
                    .background(colors.primary, RoundedCornerShape(RenoaShapes.Control))
                    .semantics { contentDescription = if (state.action == ComposerAction.Stop) "Stop" else "Send" }
                    .clickable(enabled = enabled, role = Role.Button) {
                        if (state.action == ComposerAction.Stop) viewModel.stop() else viewModel.send()
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

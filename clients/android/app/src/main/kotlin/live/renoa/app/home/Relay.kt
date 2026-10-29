package live.renoa.app.home

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import live.renoa.app.agents.AgentSummary
import live.renoa.app.badge.AgentHue
import live.renoa.app.badge.Badge
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaShapes
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.AskChoices
import live.renoa.app.ui.ButtonKind
import live.renoa.app.ui.ButtonSize
import live.renoa.app.ui.Glyphs
import live.renoa.app.ui.PathIcon
import live.renoa.app.ui.RenoaButton
import kotlin.math.abs

/**
 * One thing at a time: the most important handover, with "1 of N" to step through the rest by
 * tapping the arrows or swiping. Tapping the card opens that agent's chat; a question can be
 * answered right here. With nothing to hand over, the main agent says so and what comes next.
 */
@Composable
internal fun Relay(
    state: HomeUiState,
    next: String,
    onShow: (String) -> Unit,
    onOpenChat: (String) -> Unit,
    onAnswer: (String, String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = Renoa.colors
    val reduced = LocalReducedMotion.current
    val list = state.handovers
    val at = state.shown
    val step by rememberUpdatedState { by: Int -> list.getOrNull(at + by)?.let { onShow(it.key) }; Unit }
    val shape = RoundedCornerShape(RenoaShapes.Card)
    Box(
        modifier
            .fillMaxWidth()
            .background(colors.card, shape)
            .border(1.dp, colors.border, shape)
            .pointerInput(Unit) {
                var dx = 0f
                detectHorizontalDragGestures(onDragStart = { dx = 0f }, onDragEnd = { if (abs(dx) > 48.dp.toPx()) step(if (dx < 0) 1 else -1) }) { change, amount ->
                    dx += amount
                    change.consume()
                }
            },
    ) {
        val shown = list.getOrNull(at)
        if (shown == null) {
            Quiet(state.main, next, onOpenChat)
            return@Box
        }
        AnimatedContent(
            targetState = at to shown,
            contentKey = { it.second.key },
            transitionSpec = {
                if (reduced) {
                    EnterTransition.None togetherWith ExitTransition.None
                } else {
                    val forward = targetState.first >= initialState.first
                    (slideInHorizontally(tween(260)) { if (forward) it / 6 else -it / 6 } + fadeIn(tween(260))) togetherWith fadeOut(tween(120))
                }
            },
            label = "handover",
        ) { (index, handover) ->
            Column(
                Modifier.fillMaxWidth().clickable(onClickLabel = "Open ${handover.agent.name}") { onOpenChat(handover.agent.id) }.padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Badge(handover.agent.name, handover.agent.state, 28.dp, label = "", hue = AgentHue.of(handover.agent.id, state.roster))
                    Spacer(Modifier.width(8.dp))
                    Text(handover.agent.name, style = RenoaType.Label, color = colors.foreground)
                    Spacer(Modifier.width(6.dp))
                    Text(handover.kind.verb, Modifier.weight(1f), style = RenoaType.Caption, color = colors.mutedForeground, maxLines = 1)
                    (if (handover.kind == HandoverKind.Working) "NOW" else handover.time?.uppercase())?.let {
                        Text(it, style = RenoaType.Eyebrow, color = colors.mutedForeground)
                    }
                    if (list.size > 1) Stepper(index, list.size, step)
                }
                Text(handover.text, style = RenoaType.Heading, color = colors.foreground, maxLines = 4, overflow = TextOverflow.Ellipsis)
                handover.ask?.let { ask -> AskChoices(ask.choices, { onAnswer(handover.agent.id, it) }) }
            }
        }
    }
}

/** "‹ 2 of 5 ›", each arrow tapped across 48dp. */
@Composable
private fun Stepper(index: Int, count: Int, step: (Int) -> Unit) {
    val colors = Renoa.colors
    Row(verticalAlignment = Alignment.CenterVertically) {
        Arrow(Glyphs.BACK, "Previous", index > 0) { step(-1) }
        Text("${index + 1} of $count", style = RenoaType.Caption, color = colors.mutedForeground)
        Arrow(Glyphs.FORWARD, "Next", index < count - 1) { step(1) }
    }
}

@Composable
private fun Arrow(glyph: String, label: String, enabled: Boolean, onClick: () -> Unit) {
    Box(
        Modifier
            .size(36.dp, 48.dp)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .semantics { contentDescription = label }
            .alpha(if (enabled) 1f else 0.3f),
        contentAlignment = Alignment.Center,
    ) { PathIcon(glyph, Renoa.colors.foreground, size = 16.dp) }
}

/** Nothing to hand over: the main agent says so, says what is next, and offers a talk. */
@Composable
private fun Quiet(main: AgentSummary?, next: String, onOpenChat: (String) -> Unit) {
    val colors = Renoa.colors
    Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        if (main == null) {
            Text("Nobody is here yet.", style = RenoaType.Heading, color = colors.foreground)
            Text("Make your first agent, and they will start helping.", style = RenoaType.Body, color = colors.foregroundSoft)
            return@Column
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Badge(main.name, main.state, 28.dp, label = "", primary = true)
            Text(main.name, style = RenoaType.Label, color = colors.foreground)
        }
        Text("Nothing needs you right now.", style = RenoaType.Heading, color = colors.foreground)
        Text("$next.", style = RenoaType.Body, color = colors.foregroundSoft)
        RenoaButton("Talk to ${main.name}", { onOpenChat(main.id) }, kind = ButtonKind.Secondary, size = ButtonSize.Sm)
    }
}

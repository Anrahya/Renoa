package live.renoa.app.home

import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.clockTime
import live.renoa.app.badge.Badge
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaShapes
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.AskChoices
import live.renoa.app.ui.ButtonKind
import live.renoa.app.ui.ButtonSize
import live.renoa.app.ui.RenoaButton
import java.time.ZonedDateTime

/** What Home says while it waits to hear who is here. */
internal const val CHECKING_IN = "Checking in with your agents…"

/** What a row does with the owner's taps: open the agent's chat, answer its question, or run its failed routine again. */
internal class TalkActions(val open: (Talk) -> Unit, val answer: (String, String) -> Unit, val runAgain: (String) -> Unit)

/** When [talk] last said something: its own time today, else what its chat last showed. */
private fun Talk.time(now: ZonedDateTime): String? =
    at?.atZone(now.zone)?.takeIf { it.toLocalDate() == now.toLocalDate() }?.let { clockTime(it.toLocalTime()) } ?: agent.glance.time

/**
 * The main agent's card, pinned above everyone: its question with the answers, else its newest note
 * or the day's line, and what runs next.
 */
@Composable
internal fun MainCard(talk: Talk, roster: List<AgentSummary>, next: String, now: ZonedDateTime, lit: Boolean, act: TalkActions, modifier: Modifier = Modifier) {
    val colors = Renoa.colors
    val shape = RoundedCornerShape(RenoaShapes.Card)
    val ground by animateColorAsState(if (lit) colors.secondary else colors.card, label = "lit")
    Column(
        modifier
            .fillMaxWidth()
            .background(ground, shape)
            .border(1.5.dp, colors.foreground, shape)
            .clickable(onClickLabel = "Talk to ${talk.agent.name}") { act.open(talk) }
            .padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Header(talk, roster, if (talk.needsOwner) "NEEDS YOU" else talk.time(now)?.uppercase())
        Text(talk.line ?: "Nothing needs you right now.", style = RenoaType.Body, color = colors.foreground, maxLines = 5, overflow = TextOverflow.Ellipsis)
        when {
            talk.ask != null -> AskChoices(talk.ask.choices, { act.answer(talk.agent.id, it) })
            talk.problem != null && talk.line == talk.problem.title -> RunAgain(talk.problem.id, act)
            else -> Text("$next.", style = RenoaType.Caption, color = colors.mutedForeground)
        }
    }
}

/** An agent with something to say: its line in full, with its question's answers or a way to run a failed routine again. */
@Composable
internal fun TalkRow(talk: Talk, roster: List<AgentSummary>, now: ZonedDateTime, lit: Boolean, act: TalkActions) {
    val colors = Renoa.colors
    val ground by animateColorAsState(if (lit) colors.secondary else Color.Transparent, label = "lit")
    Column(
        Modifier
            .fillMaxWidth()
            .background(ground, RoundedCornerShape(RenoaShapes.Card))
            .clickable(onClickLabel = "Open ${talk.agent.name}") { act.open(talk) }
            .padding(horizontal = 4.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Header(talk, roster, talk.time(now))
        talk.line?.let {
            Text(it, style = RenoaType.Body, color = if (talk.needsOwner || talk.agent.glance.unread) colors.foreground else colors.foregroundSoft, maxLines = 4, overflow = TextOverflow.Ellipsis)
        }
        talk.ask?.let { ask -> AskChoices(ask.choices, { act.answer(talk.agent.id, it) }) }
        talk.problem?.takeIf { talk.line == it.title }?.let { RunAgain(it.id, act) }
    }
}

/** A quiet agent in one line: a failed routine with a way to run it again, else what it last said. */
@Composable
internal fun QuietRow(talk: Talk, roster: List<AgentSummary>, lit: Boolean, act: TalkActions) {
    val colors = Renoa.colors
    val ground by animateColorAsState(if (lit) colors.secondary else Color.Transparent, label = "lit")
    Row(
        Modifier
            .fillMaxWidth()
            .background(ground, RoundedCornerShape(RenoaShapes.Card))
            .clickable(onClickLabel = "Open ${talk.agent.name}") { act.open(talk) }
            .padding(horizontal = 4.dp, vertical = 6.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Badge(talk.agent, roster, 32.dp)
        Text(talk.agent.name, style = RenoaType.Label, color = colors.foreground)
        Text(
            talk.line.orEmpty(),
            Modifier.weight(1f),
            style = RenoaType.Caption,
            color = if (talk.problem != null) colors.stateFailed else colors.mutedForeground,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        talk.problem?.let { RunAgain(it.id, act) }
    }
}

@Composable
private fun Header(talk: Talk, roster: List<AgentSummary>, aside: String?) {
    val colors = Renoa.colors
    Row(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
        Badge(talk.agent, roster, 40.dp)
        Text(talk.agent.name, Modifier.weight(1f), style = RenoaType.Label, color = colors.foreground)
        aside?.let { Text(it, style = RenoaType.Eyebrow, color = colors.mutedForeground) }
    }
}

@Composable
private fun RunAgain(noteId: String, act: TalkActions) {
    RenoaButton("Run again", { act.runAgain(noteId) }, kind = ButtonKind.Secondary, size = ButtonSize.Sm)
}

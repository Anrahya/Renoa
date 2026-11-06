// Portions copyright 2026 Po Kai Lee and the Codync contributors.
// Licensed under the Apache License, Version 2.0; see licenses/Apache-2.0-Codync.txt.
// Adapted from Codync apps/android `BotListView.kt` (`BotRow`, `Preview`, `UnreadBadge`)
// at ed8daf11f89398e598ef1f791abc3014ded68496.
package live.renoa.app.home

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.kyant.shapes.Capsule
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.clockTime
import live.renoa.app.badge.AvatarWithStatus
import live.renoa.app.ui.AskChoices
import live.renoa.design.MenuItem
import live.renoa.design.Palette
import live.renoa.design.SecondaryButton
import live.renoa.design.ThinkingOrb
import live.renoa.design.ThinkingOrbState
import live.renoa.design.Type
import live.renoa.design.contextActions
import live.renoa.design.semibold
import java.time.ZonedDateTime

/** What Home says while it waits to hear who is here. */
internal const val CHECKING_IN = "Checking in with your agents…"

/** What a row does with the owner's taps and holds. */
internal class TalkActions(
    val open: (Talk) -> Unit,
    val answer: (String, String) -> Unit,
    val runAgain: (String) -> Unit,
    val edit: (String) -> Unit = {},
    val markRead: (String) -> Unit = {},
)

/** When [talk] last said something: its own time today, else what its chat last showed. */
private fun Talk.time(now: ZonedDateTime): String? =
    at?.atZone(now.zone)?.takeIf { it.toLocalDate() == now.toLocalDate() }?.let { clockTime(it.toLocalTime()) } ?: agent.glance.time

/**
 * One agent in Codync's flat roster: hexagonal avatar with status ring or partner badge, name and
 * time, then live activity (`ThinkingOrb`) or last message, plus inline choices or retry when needed.
 */
@Composable
internal fun AgentRow(talk: Talk, roster: List<AgentSummary>, now: ZonedDateTime, act: TalkActions) {
    val glance = talk.agent.glance
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Box(
            Modifier
                .fillMaxWidth()
                .contextActions(
                    onClick = { act.open(talk) },
                    clickLabel = "Open ${talk.agent.name}",
                    preview = { Box(Modifier.padding(horizontal = 8.dp)) { AgentRowSummary(talk, roster, now) } },
                ) {
                    listOfNotNull(
                        MenuItem("Edit profile", icon = "pencil") { act.edit(talk.agent.id) },
                        if (glance.unread || talk.note != null) {
                            MenuItem("Mark as read", icon = "checkmark.message") { act.markRead(talk.agent.id) }
                        } else {
                            null
                        },
                    )
                },
        ) {
            AgentRowSummary(talk, roster, now)
        }
        talk.ask?.let { ask ->
            AskChoices(ask.choices, { act.answer(talk.agent.id, it) }, Modifier.padding(start = 58.dp, bottom = 6.dp))
        }
        // A question comes first; its failed routine shows once it is answered.
        talk.problem?.takeIf { talk.ask == null }?.let { problem ->
            Column(Modifier.padding(start = 58.dp, bottom = 6.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (talk.line != problem.title) Text(problem.title, style = Type.footnote, color = Palette.danger)
                SecondaryButton("Run again") { act.runAgain(problem.id) }
            }
        }
    }
}

@Composable
private fun AgentRowSummary(talk: Talk, roster: List<AgentSummary>, now: ZonedDateTime) {
    val glance = talk.agent.glance
    Row(
        Modifier.fillMaxWidth().padding(vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        AvatarWithStatus(talk.agent, roster, talk.partner, 46.dp)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    talk.agent.name,
                    Modifier.weight(1f),
                    style = Type.body.semibold,
                    color = Palette.text,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Spacer(Modifier.width(8.dp))
                val aside = talk.time(now) ?: if (talk.needsOwner) "Needs you" else null
                aside?.let {
                    Text(
                        it,
                        style = Type.subheadline,
                        color = if (glance.unread || talk.needsOwner) Palette.text else Palette.tertiary,
                        maxLines = 1,
                    )
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Box(Modifier.weight(1f)) { PreviewLine(talk) }
                if (glance.unread) UnreadDot()
            }
        }
    }
}

@Composable
private fun PreviewLine(talk: Talk) {
    val glance = talk.agent.glance
    when {
        talk.needsOwner ->
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                ThinkingOrb(state = ThinkingOrbState.Listening, size = 16.dp, color = Palette.warning)
                Text(
                    talk.line ?: "Needs you",
                    style = Type.subheadline,
                    color = Palette.warning,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        talk.problem != null && talk.line == talk.problem.title ->
            Text(talk.line, style = Type.subheadline, color = Palette.danger, maxLines = 2, overflow = TextOverflow.Ellipsis)
        talk.agent.state == AgentState.Working ->
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                ThinkingOrb(size = 13.dp, color = Palette.secondary)
                Text(
                    talk.line ?: "Working…",
                    style = Type.subheadline,
                    color = Palette.secondary,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        else -> {
            val line = talk.line ?: talk.agent.purpose.takeIf { it.isNotBlank() }
            if (line != null) {
                Text(
                    line,
                    style = Type.subheadline,
                    color = if (glance.unread) Palette.text else Palette.secondary,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

/** Something new in the chat: a dot in the accent ink. */
@Composable
private fun UnreadDot() {
    Box(Modifier.size(10.dp).clip(Capsule()).background(Palette.accentFill).semantics { contentDescription = "New" })
}

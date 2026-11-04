// Portions copyright 2026 Po Kai Lee and the Codync contributors.
// Licensed under the Apache License, Version 2.0; see licenses/Apache-2.0-Codync.txt.
// The row's layout is adapted from Codync apps/android `BotListView.kt` (`BotRow`, `UnreadBadge`)
// at ed8daf11f89398e598ef1f791abc3014ded68496.
package live.renoa.app.home

import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.kyant.shapes.Capsule
import com.kyant.shapes.RoundedRectangle
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.clockTime
import live.renoa.app.badge.AgentHue
import live.renoa.app.badge.Badge
import live.renoa.app.theme.Renoa
import live.renoa.app.ui.AskChoices
import live.renoa.design.Motion
import live.renoa.design.Palette
import live.renoa.design.SecondaryButton
import live.renoa.design.Type
import live.renoa.design.semibold
import java.time.ZonedDateTime

/** What Home says while it waits to hear who is here. */
internal const val CHECKING_IN = "Checking in with your agents…"

/** What a row does with the owner's taps: open the agent's chat, answer its question, or run its failed routine again. */
internal class TalkActions(val open: (Talk) -> Unit, val answer: (String, String) -> Unit, val runAgain: (String) -> Unit)

/** When [talk] last said something: its own time today, else what its chat last showed. */
private fun Talk.time(now: ZonedDateTime): String? =
    at?.atZone(now.zone)?.takeIf { it.toLocalDate() == now.toLocalDate() }?.let { clockTime(it.toLocalTime()) } ?: agent.glance.time

/**
 * One agent: its face, its name in a touch of its colour and when it last spoke, then its line: a
 * question, a note, a failed routine, what it is doing and with whom, or what it last said; an
 * agent working with another wears that one's small face on its own. A question shows its
 * answers under it; a failed routine, once nothing is asked, a way to run it again. A row [lit]
 * from the day strip shows its ground.
 */
@Composable
internal fun AgentRow(talk: Talk, roster: List<AgentSummary>, now: ZonedDateTime, lit: Boolean, act: TalkActions) {
    val colors = Renoa.colors
    val glance = talk.agent.glance
    val ground by animateColorAsState(if (lit) Palette.surface else Color.Transparent, Motion.hover(), label = "lit")
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedRectangle(16.dp))
            .background(ground)
            .clickable(onClickLabel = "Open ${talk.agent.name}") { act.open(talk) }
            .padding(vertical = 10.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Box(Modifier.size(46.dp)) {
                Badge(talk.agent, roster, 46.dp)
                talk.partner?.let { Badge(it, roster, 22.dp, Modifier.align(Alignment.BottomEnd).offset(5.dp, 5.dp), label = "") }
            }
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    val name = AgentHue.of(talk.agent.id, roster).name(colors) ?: Palette.text
                    Text(talk.agent.name, Modifier.weight(1f), style = Type.body.semibold, color = name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Spacer(Modifier.width(8.dp))
                    val aside = if (talk.needsOwner) "Needs you" else talk.time(now)
                    aside?.let { Text(it, style = Type.subheadline, color = if (glance.unread || talk.needsOwner) Palette.text else Palette.tertiary, maxLines = 1) }
                }
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    val ink =
                        when {
                            talk.needsOwner -> Palette.attention
                            talk.problem != null && talk.line == talk.problem.title -> Palette.danger
                            talk.agent.state == AgentState.Working || !glance.unread -> Palette.secondary
                            else -> Palette.text
                        }
                    talk.line?.let { Text(it, Modifier.weight(1f), style = Type.subheadline, color = ink, maxLines = 2, overflow = TextOverflow.Ellipsis) }
                    if (glance.unread) Unread()
                }
            }
        }
        talk.ask?.let { ask -> AskChoices(ask.choices, { act.answer(talk.agent.id, it) }, Modifier.padding(start = 58.dp)) }
        // A question comes first; its failed routine shows once it is answered.
        talk.problem?.takeIf { talk.ask == null }?.let { problem ->
            Column(Modifier.padding(start = 58.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (talk.line != problem.title) Text(problem.title, style = Type.footnote, color = Palette.danger)
                SecondaryButton("Run again") { act.runAgain(problem.id) }
            }
        }
    }
}

/** Something new in the chat: a dot in the accent ink. */
@Composable
private fun Unread() {
    Box(Modifier.size(10.dp).clip(Capsule()).background(Palette.accentFill).semantics { contentDescription = "New" })
}

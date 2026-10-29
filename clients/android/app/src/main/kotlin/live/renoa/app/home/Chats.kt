package live.renoa.app.home

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.badge.AgentHue
import live.renoa.app.badge.Badge
import live.renoa.app.badge.hexPath
import live.renoa.app.theme.LocalMotionClock
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaEasing
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.AskChoices

private val RowLine = RenoaType.Body.copy(fontSize = 15.sp, lineHeight = 21.sp)

/** The Chats page: everyone with what they last said, what they are doing, or what they need, then a way to meet someone new. */
@Composable
internal fun ChatsPage(state: HomeUiState, onOpenChat: (String) -> Unit, onAnswer: (String, String) -> Unit, onNewAgent: () -> Unit) {
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 16.dp)) {
        items(state.people, key = { it.id }) { agent ->
            PersonRow(
                agent,
                AgentHue.of(agent.id, state.roster),
                agent.id == state.main?.id,
                state.agent(agent.glance.with),
                onOpen = { onOpenChat(agent.id) },
                onAnswer = { onAnswer(agent.id, it) },
                partnerHue = { AgentHue.of(it.id, state.roster) },
            )
        }
        item(key = "new") { MeetSomeoneNew(onNewAgent) }
    }
}

@Composable
private fun PersonRow(agent: AgentSummary, hue: AgentHue, main: Boolean, partner: AgentSummary?, onOpen: () -> Unit, onAnswer: (String) -> Unit, partnerHue: (AgentSummary) -> AgentHue) {
    val colors = Renoa.colors
    val glance = agent.glance
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(onClickLabel = "Open ${agent.name}", onClick = onOpen)
            .padding(horizontal = 20.dp, vertical = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(14.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Box(Modifier.size(54.dp)) {
            Badge(agent.name, agent.state, 54.dp, primary = main, hue = hue)
            if (partner != null) {
                Badge(partner.name, partner.state, 26.dp, Modifier.align(Alignment.BottomEnd).offset(6.dp, 6.dp), label = "", hue = partnerHue(partner))
            }
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(agent.name, Modifier.weight(1f), style = RenoaType.Label, color = colors.foreground)
                glance.time?.let { Text(it, style = RenoaType.Caption, color = colors.mutedForeground) }
                if (glance.unread) Box(Modifier.size(8.dp).background(colors.foreground, CircleShape).semantics { contentDescription = "New" })
            }
            val line =
                when {
                    agent.state == AgentState.Working ->
                        listOfNotNull(partner?.let { "With ${it.name}" }, glance.doing?.let { "$it…" }).joinToString(" · ").ifEmpty { "Working on it…" }
                    else -> glance.line
                }
            if (line != null) Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (agent.state == AgentState.Working && partner != null) Seam(Modifier.size(22.dp, 10.dp))
                Text(
                    line,
                    style = RowLine,
                    color = if (glance.unread || agent.state == AgentState.Stuck) colors.foreground else colors.foregroundSoft,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            glance.ask?.let { AskChoices(it.choices, onAnswer, Modifier.padding(top = 8.dp)) }
        }
    }
}

@Composable
private fun MeetSomeoneNew(onNewAgent: () -> Unit) {
    val colors = Renoa.colors
    Row(
        Modifier
            .fillMaxWidth()
            .semantics { contentDescription = "Meet someone new" }
            .clickable(role = Role.Button, onClick = onNewAgent)
            .padding(horizontal = 20.dp, vertical = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        EmptySocket(54.dp, plus = true)
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text("Meet someone new", style = RenoaType.Label, color = colors.foreground)
            Text("Give them a name and something to help with", style = RenoaType.Caption, color = colors.mutedForeground)
        }
    }
}

/** An empty tile's socket, dashed and with a plus when it makes someone new. */
@Composable
internal fun EmptySocket(size: Dp, plus: Boolean, modifier: Modifier = Modifier) {
    val colors = Renoa.colors
    Canvas(modifier.size(size)) {
        val s = this.size.minDimension / 84f
        withTransform({ translate(this.size.width / 2, this.size.height / 2); scale(s, s, Offset.Zero) }) {
            val dash = if (plus) PathEffect.dashPathEffect(floatArrayOf(4f, 4f)) else null
            drawPath(hexPath(0f, 0f, 36f, 4f), if (plus) colors.input else colors.border, style = Stroke(1.5f, pathEffect = dash))
            if (plus) {
                drawLine(colors.mutedForeground, Offset(0f, -10f), Offset(0f, 10f), 2f)
                drawLine(colors.mutedForeground, Offset(-10f, 0f), Offset(10f, 0f), 2f)
            }
        }
    }
}

/**
 * RCP drawn the landing page's way: a line between two agents with a packet running along it,
 * trailing a short tail. It holds still when motion is reduced.
 */
@Composable
private fun Seam(modifier: Modifier) {
    val colors = Renoa.colors
    val clock = LocalMotionClock.current
    val reduced = LocalReducedMotion.current
    Canvas(modifier) {
        val y = size.height / 2
        drawLine(colors.input, Offset(0f, y), Offset(size.width, y), 1.dp.toPx())
        val u = if (reduced) 0.5f else (clock.value % 1.2f) / 1.2f
        val x = size.width * RenoaEasing.EaseInOut.transform(u)
        drawLine(colors.foreground.copy(alpha = 0.5f), Offset((x - 8.dp.toPx()).coerceAtLeast(0f), y), Offset(x, y), 1.5.dp.toPx())
        drawCircle(colors.foreground, 2.5.dp.toPx(), Offset(x, y))
    }
}

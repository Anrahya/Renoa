package live.renoa.app.home

import androidx.compose.foundation.Canvas
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
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.badge.Badge
import live.renoa.app.badge.hexPath
import live.renoa.app.theme.LocalMotionClock
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaEasing
import live.renoa.app.theme.RenoaShapes
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.AskChoices
import live.renoa.app.ui.Glyphs
import live.renoa.app.ui.PathIcon
import live.renoa.app.ui.RenoaMark

private val RowLine = RenoaType.Body.copy(fontSize = 15.sp, lineHeight = 21.sp)

/**
 * Home is the owner's people, alive: a note from the lead agent about what matters now, then
 * everyone with what they last said, what they are doing, or what they need.
 */
@Composable
fun HomeScreen(
    viewModel: HomeViewModel,
    onOpenChat: (String) -> Unit,
    onNewAgent: () -> Unit,
    onChangeColours: () -> Unit,
) {
    val state by viewModel.state.collectAsState()
    val colors = Renoa.colors
    val lead = state.lead

    Box(Modifier.fillMaxSize().background(colors.background)) {
        Column(Modifier.fillMaxSize().safeDrawingPadding()) {
            Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 12.dp, top = 14.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(
                    Modifier
                        .size(44.dp)
                        .clickable(role = Role.Button, onClick = onChangeColours)
                        .semantics { contentDescription = "Renoa, change colours" },
                    contentAlignment = Alignment.CenterStart,
                ) { RenoaMark(colors.foreground, 26.dp) }
            }
            LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(bottom = 16.dp)) {
                val briefing = state.briefing
                if (briefing != null && lead != null) {
                    item(key = "briefing") { BriefingCard(lead, briefing.text, state.people) { onOpenChat(lead.id) } }
                }
                itemsIndexed(state.people, key = { _, agent -> agent.id }) { index, agent ->
                    // On the first showing the tiles drop in one after another, as an agent assembles on the landing page.
                    PersonRow(agent, state.agent(agent.glance.with), index * 90, onOpen = { onOpenChat(agent.id) }, onAnswer = { viewModel.answer(agent.id, it) })
                }
                item(key = "new") { MeetSomeoneNew(onNewAgent) }
            }
            state.notice?.let { Text(it, Modifier.padding(start = 24.dp, bottom = 6.dp), style = RenoaType.Caption, color = colors.stateFailed) }
        }
    }
}

/** Each name in the note is set in ink and weight, so the owner sees who it is about at a glance. Colour never sets text. */
private fun nameColoured(text: String, people: List<AgentSummary>, base: SpanStyle, ink: Color): AnnotatedString =
    buildAnnotatedString {
        append(text)
        addStyle(base, 0, text.length)
        for (agent in people) {
            var at = text.indexOf(agent.name)
            while (at >= 0) {
                addStyle(SpanStyle(color = ink, fontWeight = FontWeight.SemiBold), at, at + agent.name.length)
                at = text.indexOf(agent.name, at + agent.name.length)
            }
        }
    }

@Composable
private fun BriefingCard(lead: AgentSummary, text: String, people: List<AgentSummary>, onOpen: () -> Unit) {
    val colors = Renoa.colors
    val shape = RoundedCornerShape(RenoaShapes.Card)
    Row(
        Modifier
            .padding(horizontal = 16.dp, vertical = 8.dp)
            .fillMaxWidth()
            .background(colors.card.copy(alpha = 0.85f), shape)
            .border(1.dp, colors.border, shape)
            .clickable(onClickLabel = "Open ${lead.name}", onClick = onOpen)
            .padding(16.dp),
        horizontalArrangement = Arrangement.spacedBy(14.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Badge(lead.name, lead.state, 52.dp, primary = lead.primary, arriveAfterMillis = 0)
        Text(nameColoured(text, people, SpanStyle(color = colors.foregroundSoft), colors.foreground), Modifier.weight(1f).padding(top = 2.dp), style = RenoaType.Body)
    }
}

@Composable
private fun PersonRow(agent: AgentSummary, partner: AgentSummary?, arriveAfterMillis: Int, onOpen: () -> Unit, onAnswer: (String) -> Unit) {
    val colors = Renoa.colors
    val glance = agent.glance
    Row(
        Modifier
            .fillMaxWidth()
            .alpha(if (agent.state == AgentState.Asleep) 0.55f else 1f)
            .clickable(onClickLabel = "Open ${agent.name}", onClick = onOpen)
            .padding(horizontal = 20.dp, vertical = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(14.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Box(Modifier.size(54.dp)) {
            Badge(agent.name, agent.state, 54.dp, primary = agent.primary, arriveAfterMillis = arriveAfterMillis)
            if (partner != null) {
                Badge(partner.name, partner.state, 26.dp, Modifier.align(Alignment.BottomEnd).offset(6.dp, 6.dp), label = "")
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
        Canvas(Modifier.size(54.dp)) {
            val s = size.minDimension / 84f
            withTransform({ translate(size.width / 2, size.height / 2); scale(s, s, Offset.Zero) }) {
                drawPath(hexPath(0f, 0f, 36f, 4f), colors.input, style = Stroke(1.5f, pathEffect = PathEffect.dashPathEffect(floatArrayOf(4f, 4f))))
                drawLine(colors.mutedForeground, Offset(0f, -10f), Offset(0f, 10f), 2f)
                drawLine(colors.mutedForeground, Offset(-10f, 0f), Offset(10f, 0f), 2f)
            }
        }
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text("Meet someone new", style = RenoaType.Label, color = colors.foreground)
            Text("Give them a name and something to help with", style = RenoaType.Caption, color = colors.mutedForeground)
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

package live.renoa.app.home

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import live.renoa.app.agents.AgentSummary
import live.renoa.app.badge.AgentLook
import live.renoa.app.badge.Badge
import live.renoa.app.badge.HexSpiral
import live.renoa.app.badge.hexPath
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.Aura
import live.renoa.app.ui.Glyphs
import live.renoa.app.ui.PathIcon
import live.renoa.app.ui.RenoaMark

/** The design is drawn 390 wide; everything on Home scales from that. */
private const val DESIGN_WIDTH = 390f
private const val HIVE_TILE = 64f
private const val HIVE_RADIUS = 29.5f

@Composable
fun HomeScreen(
    viewModel: HomeViewModel,
    onOpenChat: (String) -> Unit,
    onNewAgent: () -> Unit,
    onChangeColours: () -> Unit,
) {
    val layout by viewModel.layout.collectAsState()
    val colors = Renoa.colors
    BoxWithConstraints(Modifier.fillMaxSize().background(colors.background)) {
        val unit = maxWidth / DESIGN_WIDTH
        val tones = layout.stage.map { remember(it.name) { AgentLook.forName(it.name).main } }
        Aura(tones.getOrNull(0) ?: colors.signature, Modifier.offset(unit * -60, unit * 120).size(unit * 260, unit * 220), alpha = 0.28f)
        Aura(tones.getOrNull(1) ?: colors.signature, Modifier.offset(unit * 190, unit * 90).size(unit * 240, unit * 200), alpha = 0.22f, offsetSeconds = 8f)

        Column(Modifier.fillMaxSize().safeDrawingPadding().verticalScroll(rememberScrollState())) {
            Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 12.dp, top = 14.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(
                    Modifier
                        .size(44.dp)
                        .clickable(role = Role.Button, onClick = onChangeColours)
                        .semantics { contentDescription = "Renoa, change colours" },
                    contentAlignment = Alignment.CenterStart,
                ) { RenoaMark(colors.foreground, 26.dp) }
            }
            Stage(layout.stage, unit, onOpenChat)
            Hive(layout, unit, onOpenChat, onNewAgent, viewModel::toggleAsleep)
        }
    }
}

@Composable
private fun Stage(agents: List<AgentSummary>, unit: Dp, onOpenChat: (String) -> Unit) {
    val slots = HomeLayout.slots(agents.size)
    Box(Modifier.fillMaxWidth().height(unit * HomeLayout.stageHeight(agents.size)).semantics { contentDescription = "Busy now" }) {
        agents.zip(slots).forEachIndexed { index, (agent, slot) ->
            val look = remember(agent.name) { AgentLook.forName(agent.name) }
            Column(
                Modifier
                    .offset(unit * slot.left, unit * slot.top)
                    .width(unit * slot.size)
                    .clickable(onClickLabel = "Open ${agent.name}") { onOpenChat(agent.id) },
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Badge(look, agent.state, unit * slot.size, label = agent.name)
                Text(
                    agent.name,
                    style = if (index == 0) RenoaType.Label.copy(fontSize = 16.sp) else RenoaType.Label,
                    color = Renoa.colors.foreground,
                    textAlign = TextAlign.Center,
                )
            }
        }
    }
}

@Composable
private fun Hive(layout: HomeLayout, unit: Dp, onOpenChat: (String) -> Unit, onNewAgent: () -> Unit, onToggleAsleep: () -> Unit) {
    val colors = Renoa.colors
    val seats = remember(layout.hive.size) {
        (0..layout.hive.size).map { HexSpiral.centre(HexSpiral.seat(it), HIVE_RADIUS) }
    }
    val minX = seats.minOf { it.x } - HIVE_TILE / 2
    val minY = seats.minOf { it.y } - HIVE_TILE / 2
    val width = seats.maxOf { it.x } + HIVE_TILE / 2 - minX
    val height = seats.maxOf { it.y } + HIVE_TILE / 2 - minY

    Column(Modifier.fillMaxWidth().padding(top = 8.dp, bottom = 32.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        if (layout.asleepCount > 0) {
            Row(
                Modifier
                    .align(Alignment.End)
                    .padding(end = 16.dp)
                    .height(32.dp)
                    .background(colors.secondary, RoundedCornerShape(16.dp))
                    .clickable(role = Role.Button, onClick = onToggleAsleep)
                    .padding(horizontal = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                PathIcon(Glyphs.MOON, colors.foregroundSoft, size = 14.dp, strokeWidth = 1.8f)
                Text(
                    if (layout.asleepHidden) "Show ${layout.asleepCount} asleep" else "${layout.asleepCount} asleep · tidy up",
                    style = RenoaType.Caption,
                    color = colors.foregroundSoft,
                )
            }
        }
        Box(Modifier.width(unit * width).height(unit * height).padding(top = 8.dp)) {
            layout.hive.forEachIndexed { i, agent ->
                val look = remember(agent.name) { AgentLook.forName(agent.name) }
                Box(
                    Modifier
                        .offset(unit * (seats[i].x - minX - HIVE_TILE / 2), unit * (seats[i].y - minY - HIVE_TILE / 2))
                        .clickable(onClickLabel = "Open ${agent.name}") { onOpenChat(agent.id) },
                ) { Badge(look, agent.state, unit * HIVE_TILE, mini = true, label = agent.name) }
            }
            val last = seats.last()
            Box(
                Modifier
                    .offset(unit * (last.x - minX - HIVE_TILE / 2), unit * (last.y - minY - HIVE_TILE / 2))
                    .size(unit * HIVE_TILE)
                    .semantics { contentDescription = "Make a new agent" }
                    .clickable(role = Role.Button, onClick = onNewAgent),
                contentAlignment = Alignment.Center,
            ) { NewSeat() }
        }
    }
}

@Composable
private fun NewSeat() {
    val colors = Renoa.colors
    Canvas(Modifier.fillMaxSize()) {
        val s = size.minDimension / 84f
        withTransform({ translate(size.width / 2, size.height / 2); scale(s, s, Offset.Zero) }) {
            drawPath(hexPath(0f, 0f, 36f, 4f), colors.input, style = Stroke(1.5f, pathEffect = PathEffect.dashPathEffect(floatArrayOf(4f, 4f))))
            drawLine(colors.mutedForeground, Offset(0f, -10f), Offset(0f, 10f), 2f)
            drawLine(colors.mutedForeground, Offset(-10f, 0f), Offset(10f, 0f), 2f)
        }
    }
}

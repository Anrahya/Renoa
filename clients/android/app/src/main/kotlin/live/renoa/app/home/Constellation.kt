package live.renoa.app.home

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Text
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.key
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.DpOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.min
import live.renoa.app.agents.AgentSummary
import live.renoa.app.badge.AgentHue
import live.renoa.app.badge.Badge
import live.renoa.app.theme.LocalMotionClock
import live.renoa.app.theme.LocalReducedMotion
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaEasing
import live.renoa.app.theme.RenoaType
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.sin

/** Where the seats sit round the main agent, in degrees clockwise from the right: the sides fill first, so the space under her stays open. */
private val SEAT_ANGLES = listOf(-30.0, 210.0, 30.0, 150.0, -90.0, 90.0)

/** What a seat holds: an agent, the way to meet someone new, the rest of a long roster, or nothing yet. */
internal sealed interface SeatHolds {
    data class Agent(val agent: AgentSummary) : SeatHolds

    data object New : SeatHolds

    data class More(val count: Int) : SeatHolds

    data object Empty : SeatHolds
}

/**
 * The seats in order. While everyone fits, each keeps the seat of its place on the roster, so no
 * tile moves when an agent's state changes. A roster longer than the seats seats in Home's order,
 * so whoever needs the owner is seated, and keeps the last seat for the rest.
 */
internal fun seats(roster: List<AgentSummary>): List<SeatHolds> {
    val others = if (roster.size <= SEAT_ANGLES.size) roster else homeOrder(roster)
    val n = SEAT_ANGLES.size
    val seated = if (others.size > n) others.take(n - 1).map { SeatHolds.Agent(it) } + SeatHolds.More(others.size - n + 1) else others.map { SeatHolds.Agent(it) }
    return List(n) { i -> seated.getOrNull(i) ?: if (i == seated.size) SeatHolds.New else SeatHolds.Empty }
}

/**
 * The main agent in the middle with the others on hex seats round her, each wired to her. While
 * two agents work together, a packet runs along the wire between them. Tapping anyone opens
 * their chat.
 */
@Composable
internal fun Constellation(
    state: HomeUiState,
    assemble: Boolean,
    onOpenChat: (String) -> Unit,
    onNewAgent: () -> Unit,
    onMore: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = Renoa.colors
    val clock = LocalMotionClock.current
    val reduced = LocalReducedMotion.current
    val main = state.main
    val holds = seats(state.roster.filter { it.id != main?.id })
    val pair = talking(state.roster)

    BoxWithConstraints(modifier.fillMaxWidth().heightIn(min = 200.dp), contentAlignment = Alignment.Center) {
        val hub = min(min(maxWidth * 0.28f, maxHeight * 0.4f), 104.dp)
        val seat = hub * 0.62f
        // Far enough out to clear the hub, near enough that the top and bottom seats and the side ones fit.
        val reach = min(min(maxHeight / 2 - seat / 2 - 4.dp, (maxWidth / 2 - seat / 2 - 8.dp) / 0.866f), hub * 1.55f)
        val at = SEAT_ANGLES.map { DpOffset(reach * cos(it * PI / 180).toFloat(), reach * sin(it * PI / 180).toFloat()) }
        fun place(id: String): DpOffset? = if (id == main?.id) DpOffset.Zero else holds.indexOfFirst { it is SeatHolds.Agent && it.agent.id == id }.takeIf { it >= 0 }?.let { at[it] }
        val wire = pair?.let { (from, to) -> place(from.id)?.let { a -> place(to.id)?.let { b -> Triple(a, b, AgentHue.of(from.id, state.roster).face(colors) ?: colors.foreground) } } }

        Canvas(Modifier.fillMaxSize()) {
            val centre = Offset(size.width / 2, size.height / 2)
            fun Dp.px() = toPx()
            fun DpOffset.px() = centre + Offset(x.px(), y.px())
            holds.forEachIndexed { i, held ->
                if (held is SeatHolds.Agent && main != null) drawLine(colors.border, centre, at[i].px(), 1.dp.toPx())
            }
            wire?.let { (from, to, ink) -> drawPacket(from.px(), to.px(), ink, if (reduced) 0.5f else (clock.value % 1.5f) / 1.5f, 13.dp.toPx(), 3.2.dp.toPx()) }
        }

        holds.forEachIndexed { i, held ->
            val where = Modifier.align(Alignment.Center).offset(at[i].x, at[i].y)
            when (held) {
                is SeatHolds.Agent ->
                    // Keyed by agent, so a tile's own motion stays with it if it changes seats.
                    key(held.agent.id) {
                        Box(where.minimumInteractiveComponentSize().clickable(onClickLabel = "Open ${held.agent.name}") { onOpenChat(held.agent.id) }, contentAlignment = Alignment.Center) {
                            Badge(held.agent, state.roster, seat, arriveAfterMillis = if (assemble) 120 + i * 90 else null)
                        }
                    }
                SeatHolds.New ->
                    Box(where.minimumInteractiveComponentSize().clickable(role = Role.Button, onClick = onNewAgent).semantics { contentDescription = "Meet someone new" }, contentAlignment = Alignment.Center) {
                        EmptySocket(seat, plus = true)
                    }
                is SeatHolds.More ->
                    Box(where.minimumInteractiveComponentSize().clickable(role = Role.Button, onClick = onMore).semantics { contentDescription = "${held.count} more, in Chats" }, contentAlignment = Alignment.Center) {
                        EmptySocket(seat, plus = false)
                        Text("+${held.count}", style = RenoaType.Label, color = colors.foregroundSoft)
                    }
                SeatHolds.Empty -> EmptySocket(seat, plus = false, where)
            }
        }

        if (main != null) {
            Box(Modifier.align(Alignment.Center).clickable(onClickLabel = "Talk to ${main.name}") { onOpenChat(main.id) }) {
                Badge(main, state.roster, hub, arriveAfterMillis = if (assemble) 0 else null)
            }
        } else {
            EmptySocket(hub, plus = true, Modifier.align(Alignment.Center).clickable(role = Role.Button, onClick = onNewAgent).semantics { contentDescription = "Make your first agent" })
        }
    }
}

/** A packet running from [from] to [to] at [u] of the way, eased, trailing a short tail. */
private fun androidx.compose.ui.graphics.drawscope.DrawScope.drawPacket(from: Offset, to: Offset, ink: Color, u: Float, tail: Float, radius: Float) {
    val e = RenoaEasing.EaseInOut.transform(u)
    val p = from + (to - from) * e
    val length = hypot(to.x - from.x, to.y - from.y).coerceAtLeast(1f)
    val back = (from - to) * (tail / length)
    drawLine(ink.copy(alpha = 0.55f), p, p + back, 1.6.dp.toPx())
    drawCircle(ink, radius, p)
}

/** The line under the constellation: who is working together right now, or how to talk to the main agent. */
@Composable
internal fun TalkLine(state: HomeUiState, onOpenChat: (String) -> Unit, modifier: Modifier = Modifier) {
    val colors = Renoa.colors
    val pair = talking(state.roster)
    val main = state.main
    if (pair != null) {
        val (from, to) = pair
        Row(
            modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(onClickLabel = "Open ${from.name}") { onOpenChat(from.id) }.padding(horizontal = 20.dp, vertical = 10.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            val ink = AgentHue.of(from.id, state.roster).face(colors) ?: colors.foreground
            Canvas(Modifier.size(8.dp)) { drawCircle(ink) }
            Text(
                "${from.name} and ${to.name} · ${from.glance.doing ?: "Working on it"}…",
                style = RenoaType.Caption,
                color = colors.foregroundSoft,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    } else {
        val hint =
            when {
                main == null -> "Make your first agent to begin"
                state.roster.size == 1 -> "Tap ${main.name} to talk"
                else -> "Tap ${main.name} to talk, or anyone else for their chat"
            }
        Text(hint, modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 10.dp), style = RenoaType.Caption, color = colors.mutedForeground, textAlign = TextAlign.Center)
    }
}

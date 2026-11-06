// Portions copyright 2026 Po Kai Lee and the Codync contributors.
// Licensed under the Apache License, Version 2.0; see licenses/Apache-2.0-Codync.txt.
// Adapted from Codync apps/android `BotAvatar.kt` at ed8daf11f89398e598ef1f791abc3014ded68496.
package live.renoa.app.badge

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import live.renoa.app.agents.AgentSummary
import live.renoa.design.Palette
import live.renoa.design.Symbol

/** The roster's hexagonal badge, with Codync's ringed status indicator or partner badge on its corner. */
@Composable
fun AvatarWithStatus(
    agent: AgentSummary,
    roster: List<AgentSummary>,
    partner: AgentSummary? = null,
    size: Dp = 46.dp,
    arriveAfterMillis: Int? = null,
) {
    Box(Modifier.size(size)) {
        Badge(agent, roster, size, arriveAfterMillis = arriveAfterMillis)
        when {
            partner != null ->
                Box(Modifier.align(Alignment.BottomEnd)) {
                    Badge(partner, roster, size * .48f, label = "")
                }
            agent.state.needsOwner ->
                Box(
                    Modifier
                        .size(size * .36f)
                        .align(Alignment.BottomEnd)
                        .statusRing()
                        .background(Palette.warning, CircleShape),
                    contentAlignment = Alignment.Center,
                ) {
                    Symbol("exclamationmark", size = size * .24f, tint = Color.White)
                }
            agent.glance.unread ->
                Box(
                    Modifier
                        .size(size * .28f)
                        .align(Alignment.BottomEnd)
                        .statusRing()
                        .background(Palette.accentFill, CircleShape),
                )
        }
    }
}

/** A 2 dp ring in the ground colour around a status dot, centred on its edge. */
@Composable
private fun Modifier.statusRing(): Modifier {
    val color = Palette.background
    return drawWithContent {
        drawContent()
        drawCircle(color, size.minDimension / 2, style = Stroke(2.dp.toPx()))
    }
}

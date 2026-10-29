package live.renoa.app.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.expandVertically
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import live.renoa.app.agents.Link
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaType

/** What the owner reads while the connection to Renoa is down. */
const val LOST_WORDS = "Can't reach Renoa. Trying again…"

/** A quiet line while the connection to Renoa is down, gone again once it is back. */
@Composable
fun LinkLine(link: Link, modifier: Modifier = Modifier) {
    val colors = Renoa.colors
    AnimatedVisibility(link == Link.Lost, modifier, enter = fadeIn() + expandVertically(), exit = fadeOut() + shrinkVertically()) {
        Row(
            Modifier.semantics { liveRegion = LiveRegionMode.Polite },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Canvas(Modifier.size(7.dp)) { drawCircle(colors.stateFailed) }
            Text(LOST_WORDS, style = RenoaType.Caption, color = colors.mutedForeground)
        }
    }
}

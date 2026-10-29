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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import kotlinx.coroutines.delay
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

/** How long the connection stays down before the line shows, so a moment's drop goes unremarked. */
const val LOST_AFTER_MILLIS = 2_000L

/** A quiet line once the connection to Renoa has been down a moment, gone again once it is back. */
@Composable
fun LinkLine(link: Link, modifier: Modifier = Modifier) {
    val colors = Renoa.colors
    var shown by remember { mutableStateOf(false) }
    LaunchedEffect(link) {
        if (link == Link.Lost) delay(LOST_AFTER_MILLIS)
        shown = link == Link.Lost
    }
    AnimatedVisibility(shown, enter = fadeIn() + expandVertically(), exit = fadeOut() + shrinkVertically()) {
        Row(
            // The space around it comes and goes with it.
            modifier.semantics { liveRegion = LiveRegionMode.Polite },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Canvas(Modifier.size(7.dp)) { drawCircle(colors.stateFailed) }
            Text(LOST_WORDS, style = RenoaType.Caption, color = colors.mutedForeground)
        }
    }
}

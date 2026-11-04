/*
 * Adapted from Codync's ConnectionRequestCard.kt and ServiceLogo (MarketRows.kt), commit
 * ed8daf11f89398e598ef1f791abc3014ded68496, Copyright 2026 Po Kai Lee, under the Apache License,
 * Version 2.0 (see clients/android/licenses/Apache-2.0-Codync.txt). Changed: Renoa's theme, a
 * setup link instead of a chat entry, and a service mark instead of a fetched favicon.
 */
package live.renoa.app.connect

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import live.renoa.app.ui.ButtonKind
import live.renoa.app.ui.ButtonSize
import live.renoa.app.ui.PathIcon
import live.renoa.app.ui.RenoaButton
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaType
import live.renoa.rcp.SetupLink
import live.renoa.rcp.SetupLinkKind

/**
 * A setup link waiting for the owner: what it connects, and a Connect button that opens the link.
 * [problem] says why the last tap did not open it. Not now puts it away for good.
 */
@Composable
fun ConnectCard(link: SetupLink, onConnect: () -> Unit, onNotNow: () -> Unit, modifier: Modifier = Modifier, problem: String? = null) {
    val colors = Renoa.colors
    val shape = RoundedCornerShape(20.dp)
    // A card, not a banner: full width on a phone, capped on wider screens.
    Column(
        modifier
            .widthIn(max = 400.dp)
            .fillMaxWidth()
            .background(colors.card, shape)
            .border(1.dp, colors.border, shape)
            .padding(horizontal = 16.dp, vertical = 14.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            ServiceLogo(link)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(link.label, style = RenoaType.Label, color = colors.foreground, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(link.kind.words, style = RenoaType.Caption, color = colors.mutedForeground, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
            RenoaButton("Not now", onNotNow, kind = ButtonKind.Secondary, size = ButtonSize.Sm)
            RenoaButton("Connect", onConnect, size = ButtonSize.Sm)
        }
        problem?.let { Text(it, style = RenoaType.Caption, color = colors.stateFailed) }
    }
}

/** What the owner does at the other end of the link. */
private val SetupLinkKind.words: String
    get() =
        when (this) {
            SetupLinkKind.Authorization -> "Sign in there to let Renoa use it."
            SetupLinkKind.Credentials -> "Enter its key on a private Renoa page."
        }

/** The service's mark on a tile, or the first letter of what the link connects. */
@Composable
private fun ServiceLogo(link: SetupLink) {
    val colors = Renoa.colors
    val mark = serviceMark(link.url, link.label)
    Box(
        Modifier.size(46.dp).clip(RoundedCornerShape(12.dp)).background(colors.muted).clearAndSetSemantics {},
        contentAlignment = Alignment.Center,
    ) {
        if (mark != null) {
            PathIcon(mark.path, colors.foreground, size = 26.dp)
        } else {
            Text(link.label.firstOrNull()?.uppercase() ?: "?", style = RenoaType.Heading, color = colors.foreground)
        }
    }
}

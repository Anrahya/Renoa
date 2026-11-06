/*
 * Adapted from Codync's ConnectionRequestCard.kt and ServiceLogo (MarketRows.kt), commit
 * ed8daf11f89398e598ef1f791abc3014ded68496, Copyright 2026 Po Kai Lee, under the Apache License,
 * Version 2.0 (see clients/android/licenses/Apache-2.0-Codync.txt). Changed: a setup link instead
 * of a chat entry, and a service mark instead of a fetched favicon.
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
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.kyant.shapes.RoundedRectangle
import live.renoa.design.Palette
import live.renoa.design.PrimaryButton
import live.renoa.design.SecondaryButton
import live.renoa.design.Type
import live.renoa.design.semibold
import live.renoa.rcp.SetupLink
import live.renoa.rcp.SetupLinkKind

/**
 * A setup link waiting for the owner: what it connects, and a Connect button that opens the link.
 * [problem] says why the last tap did not open it. Not now puts it away for good.
 */
@Composable
fun ConnectCard(link: SetupLink, onConnect: () -> Unit, onNotNow: () -> Unit, modifier: Modifier = Modifier, problem: String? = null) {
    val shape = RoundedRectangle(20.dp)
    // A card, not a banner: full width on a phone, capped on wider screens.
    Column(
        modifier
            .widthIn(max = 400.dp)
            .fillMaxWidth()
            .clip(shape)
            .background(Palette.surface)
            .border(1.dp, Palette.border, shape)
            .padding(horizontal = 16.dp, vertical = 14.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            ServiceLogo(link.url, link.label)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(link.label, style = Type.body.semibold, color = Palette.text, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(link.kind.words, style = Type.subheadline, color = Palette.secondary, maxLines = 2, overflow = TextOverflow.Ellipsis)
                // Where Connect goes, since the label is the link maker's word for it.
                linkHost(link.url)?.let { Text("Opens $it", style = Type.caption, color = Palette.tertiary, maxLines = 1, overflow = TextOverflow.Ellipsis) }
            }
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
            SecondaryButton("Not now", action = onNotNow)
            PrimaryButton("Connect", action = onConnect)
        }
        problem?.let { Text(it, style = Type.footnote, color = Palette.danger) }
    }
}

/** What the owner does at the other end of the link. */
private val SetupLinkKind.words: String
    get() =
        when (this) {
            SetupLinkKind.Authorization -> "Sign in there to let Renoa use it."
            SetupLinkKind.Credentials -> "Enter its key on a private Renoa page."
        }

/** The mark of the service at [url] on a tile, or the first letter of [label], what it connects. */
@Composable
internal fun ServiceLogo(url: String, label: String, size: Dp = 46.dp) {
    val mark = serviceMark(url)
    Box(
        Modifier
            .size(size)
            .clip(RoundedRectangle(size * 12 / 46))
            .background(Palette.bubbleAgent)
            .clearAndSetSemantics {},
        contentAlignment = Alignment.Center,
    ) {
        if (mark != null) {
            ServiceGlyph(mark.path, Palette.text, size = size * 26 / 46)
        } else {
            Text(
                label.firstOrNull()?.uppercase() ?: "?",
                style = Type.system(size.value * 0.4f, FontWeight.SemiBold),
                color = Palette.text,
            )
        }
    }
}

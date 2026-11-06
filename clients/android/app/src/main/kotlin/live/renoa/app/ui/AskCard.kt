// Portions copyright 2026 Po Kai Lee and the Codync contributors.
// Licensed under the Apache License, Version 2.0; see licenses/Apache-2.0-Codync.txt.
// Adapted from Codync apps/android `PermissionCard.kt` at ed8daf11f89398e598ef1f791abc3014ded68496.
package live.renoa.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import com.kyant.shapes.Capsule
import com.kyant.shapes.RoundedRectangle
import live.renoa.app.agents.Ask
import live.renoa.design.Hairline
import live.renoa.design.Palette
import live.renoa.design.PrimaryButton
import live.renoa.design.SecondaryButton
import live.renoa.design.Type
import live.renoa.design.semibold

/**
 * The answers an agent offers on Home, as capsule buttons. The first is the one it expects, so it
 * wears the accent fill; the rest are secondary.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun AskChoices(choices: List<String>, onAnswer: (String) -> Unit, modifier: Modifier = Modifier) {
    FlowRow(
        modifier,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        choices.forEachIndexed { index, choice ->
            if (index == 0) {
                PrimaryButton(choice) { onAnswer(choice) }
            } else {
                SecondaryButton(choice) { onAnswer(choice) }
            }
        }
    }
}

/** A question only the owner can answer, sitting above the composer in Codync's choice-card style. */
@Composable
fun AskCard(ask: Ask, onAnswer: (String) -> Unit, modifier: Modifier = Modifier) {
    Column(
        modifier
            .fillMaxWidth()
            .clip(RoundedRectangle(22.dp))
            .background(Palette.bubbleAgent)
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(
            verticalAlignment = Alignment.Top,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Box(Modifier.padding(top = 7.dp).size(7.dp).clip(Capsule()).background(Palette.warning))
            Text(ask.question, Modifier.weight(1f), style = Type.subheadline.semibold, color = Palette.text)
        }
        if (ask.choices.isNotEmpty()) {
            Column(Modifier.fillMaxWidth().clip(RoundedRectangle(14.dp)).background(Palette.background)) {
                ask.choices.forEachIndexed { index, choice ->
                    if (index > 0) Hairline(Modifier.padding(horizontal = 14.dp))
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .heightIn(min = 46.dp)
                            .clickable(role = Role.Button) { onAnswer(choice) }
                            .padding(horizontal = 14.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            choice,
                            Modifier.weight(1f),
                            style = if (index == 0) Type.body.semibold else Type.body,
                            color = Palette.text,
                        )
                    }
                }
            }
        }
    }
}

// Portions copyright 2026 Po Kai Lee and the Codync contributors.
// Licensed under the Apache License, Version 2.0; see licenses/Apache-2.0-Codync.txt.
// Adapted from Codync apps/android `ChatRows.kt` (`IntroCard`) at ed8daf11f89398e598ef1f791abc3014ded68496.
package live.renoa.app.chat

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import live.renoa.app.badge.Badge
import live.renoa.design.Palette
import live.renoa.design.SecondaryButton
import live.renoa.design.Type
import live.renoa.design.semibold

/** What the owner might first ask the main agent: everyday help, not set-up. */
val MAIN_STARTERS = listOf("What can you help me with?", "Plan my meals for this week", "Remind me about something later")

/** What the owner might first ask any other agent. */
val AGENT_STARTERS = listOf("What can you do?", "What should I tell you about me?")

/**
 * A chat with nothing in it yet: the agent, what it is for, and a few ways to start. A starter
 * fills the composer rather than sending, so the owner can make it their own first. The starters
 * step aside while the owner is typing, so a tap never replaces their words.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun ChatStart(state: ChatUiState, onStarter: (String) -> Unit, modifier: Modifier = Modifier) {
    val agent = state.agent ?: return
    Column(
        modifier.fillMaxWidth().padding(horizontal = 24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Badge(agent.name, agent.state, 72.dp, primary = state.main, look = agent.look, label = "")
        Text(
            "Say hello to ${agent.name}",
            Modifier.semantics { heading() },
            style = Type.title2.semibold,
            color = Palette.text,
            textAlign = TextAlign.Center,
        )
        val subtitle = agent.purpose.takeIf { it.isNotBlank() } ?: "Tell it what you need. You'll get a notification when it's done or needs you."
        Text(subtitle, style = Type.subheadline, color = Palette.secondary, textAlign = TextAlign.Center)
        if (state.draft.isBlank()) {
            FlowRow(
                Modifier.padding(top = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                (if (state.main) MAIN_STARTERS else AGENT_STARTERS).forEach { starter ->
                    SecondaryButton(starter) { onStarter(starter) }
                }
            }
        }
    }
}

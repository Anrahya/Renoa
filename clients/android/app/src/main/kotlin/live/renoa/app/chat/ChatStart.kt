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
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import live.renoa.app.badge.Badge
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaType
import live.renoa.app.ui.ButtonKind
import live.renoa.app.ui.ButtonSize
import live.renoa.app.ui.RenoaButton

/** What the owner might first ask the main agent: everyday help, not set-up. */
val MAIN_STARTERS = listOf("What can you help me with?", "Plan my meals for this week", "Remind me about something later")

/** What the owner might first ask any other agent. */
val AGENT_STARTERS = listOf("What can you do?", "What should I tell you about me?")

/**
 * A chat with nothing in it yet: the agent, what it is for, and a few ways to start. A starter
 * fills the composer rather than sending, so the owner can make it their own first.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun ChatStart(state: ChatUiState, onStarter: (String) -> Unit, modifier: Modifier = Modifier) {
    val agent = state.agent ?: return
    val colors = Renoa.colors
    Column(modifier.fillMaxWidth().padding(horizontal = 28.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Badge(agent.name, agent.state, 104.dp, primary = state.main, hue = state.hue, label = "")
        Text("Say hello to ${agent.name}", style = RenoaType.Heading, color = colors.foreground, textAlign = TextAlign.Center)
        agent.purpose.takeIf { it.isNotBlank() }?.let { Text(it, style = RenoaType.Body, color = colors.mutedForeground, textAlign = TextAlign.Center) }
        FlowRow(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally)) {
            (if (state.main) MAIN_STARTERS else AGENT_STARTERS).forEach { starter ->
                RenoaButton(starter, { onStarter(starter) }, kind = ButtonKind.Secondary, size = ButtonSize.Sm)
            }
        }
    }
}

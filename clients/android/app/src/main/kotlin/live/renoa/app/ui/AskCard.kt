package live.renoa.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import live.renoa.app.agents.Ask
import live.renoa.app.theme.Renoa
import live.renoa.app.theme.RenoaShapes
import live.renoa.app.theme.RenoaType

/**
 * The answers an agent offers, as buttons. The first is the one it expects, so it is the solid
 * ink button; the rest are secondary.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun AskChoices(choices: List<String>, onAnswer: (String) -> Unit, modifier: Modifier = Modifier) {
    FlowRow(modifier, horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        choices.forEachIndexed { index, choice ->
            RenoaButton(choice, onClick = { onAnswer(choice) }, kind = if (index == 0) ButtonKind.Primary else ButtonKind.Secondary, size = ButtonSize.Sm)
        }
    }
}

/** A question only the owner can answer, sitting above the composer until it is answered. */
@Composable
fun AskCard(ask: Ask, onAnswer: (String) -> Unit, modifier: Modifier = Modifier) {
    val colors = Renoa.colors
    val shape = RoundedCornerShape(RenoaShapes.Card)
    Column(
        modifier
            .fillMaxWidth()
            .background(colors.card, shape)
            .border(1.dp, colors.border, shape)
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
        horizontalAlignment = Alignment.Start,
    ) {
        Text(ask.question, style = RenoaType.Body, color = colors.foreground)
        AskChoices(ask.choices, onAnswer)
    }
}

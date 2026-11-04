package live.renoa.app.home

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import live.renoa.app.agents.AgentApp
import live.renoa.app.connect.ServiceLogo
import live.renoa.design.CardSection
import live.renoa.design.DesignSwitch
import live.renoa.design.Palette
import live.renoa.design.SecondaryButton
import live.renoa.design.Type
import live.renoa.design.semibold

/**
 * What the app is called, in plain words: its plugin's name, else its connection's, without the
 * word mcp and with spaces for dashes, such as Exa for exa-mcp.
 */
internal val AgentApp.label: String
    get() {
        val words = (name ?: connection).split('-', '_', '.', ' ').filter { it.isNotEmpty() && !it.equals("mcp", ignoreCase = true) }
        return words.joinToString(" ").replaceFirstChar { it.titlecase() }.ifEmpty { connection }
    }

/**
 * The apps connected to Renoa, each with a switch for whether the main agent uses it. Connecting a
 * new one happens in a chat, which [onAsk] opens.
 */
@Composable
internal fun AppsPage(state: HomeUiState, onSwitch: (String, Boolean) -> Unit, onAsk: (String) -> Unit) {
    val main = state.main
    val apps = state.apps ?: return Empty("Checking which apps are connected…")
    Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
        if (apps.isEmpty()) {
            Empty("No apps are connected yet.")
        } else {
            CardSection(footer = "Turning an app off keeps its sign-in, so turning it on again is instant.") {
                for (app in apps) {
                    row {
                        DesignSwitch(state.isOn(app), { onSwitch(app.connection, it) }, enabled = app.connection !in state.appSwitching) {
                            ServiceLogo(app.endpoint, app.label, 34.dp)
                            Column(Modifier.weight(1f)) {
                                Text(app.label, style = Type.subheadline.semibold, color = Palette.text, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                        }
                    }
                }
            }
        }
        if (main != null) {
            Text("To connect another app, ask ${main.name}.", Modifier.padding(horizontal = 2.dp), style = Type.subheadline, color = Palette.secondary)
            SecondaryButton("Ask ${main.name}") { onAsk(main.id) }
        }
    }
}

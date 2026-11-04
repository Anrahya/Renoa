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
import live.renoa.app.connect.serviceMark
import live.renoa.design.CardSection
import live.renoa.design.DesignSwitch
import live.renoa.design.Palette
import live.renoa.design.SecondaryButton
import live.renoa.design.Type
import live.renoa.design.semibold

/**
 * What the app is called, in plain words: the service's own name when its address is one the app
 * knows, else its plugin's name, else its connection's, without the word mcp and with spaces for
 * dashes, such as Exa for exa-mcp.
 */
internal val AgentApp.label: String
    get() {
        serviceMark(endpoint)?.let { return it.title }
        val words = (name ?: connection).split('-', '_', '.', ' ').filter { it.isNotEmpty() && !it.equals("mcp", ignoreCase = true) }
        return words.joinToString(" ").replaceFirstChar { it.titlecase() }.ifEmpty { connection }
    }

/**
 * The apps connected to Renoa, each with a switch for whether the main agent uses it. Connecting a
 * new one happens in a chat, which [onAsk] opens.
 */
@Composable
internal fun AppsPage(state: HomeUiState, onSwitch: (String, Boolean) -> Unit, onAsk: (String) -> Unit, onRetry: () -> Unit) {
    val main = state.main
    if (HomePage.Apps in state.unread) return Unread("The apps could not be read.", onRetry)
    val apps = state.apps ?: return Empty("Checking which apps are connected…")
    Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
        if (apps.isEmpty()) {
            Empty("No apps are connected yet.")
        } else {
            CardSection(footer = "Turning an app off doesn't disconnect it. Turn it back on any time.") {
                for (app in apps) {
                    row {
                        DesignSwitch(state.isOn(app), { onSwitch(app.connection, it) }, enabled = app.connection !in state.appSwitching) {
                            ServiceLogo(app.endpoint, app.label, 34.dp)
                            Text(app.label, Modifier.weight(1f), style = Type.subheadline.semibold, color = Palette.text, maxLines = 1, overflow = TextOverflow.Ellipsis)
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

/** A page whose last read failed: what failed, and a way to read it again. */
@Composable
internal fun Unread(words: String, onRetry: () -> Unit) {
    Column(Modifier.padding(horizontal = 8.dp, vertical = 24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(words, style = Type.body, color = Palette.secondary)
        SecondaryButton("Try again", action = onRetry)
    }
}

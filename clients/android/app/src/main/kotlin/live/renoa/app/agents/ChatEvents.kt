package live.renoa.app.agents

import live.renoa.rcp.AgentMessageKind
import live.renoa.rcp.CommandEnvelope
import live.renoa.rcp.CommandInput

/** The surface a node runs the Host's automation schedule through, as `docs/rcp-v0.md` names it. */
internal const val AUTOMATIONS_SURFACE = "automations"

/** A routine's name in the first line the Host puts on its run: `(Scheduled run "Name", …)` or `(Requested run of "Name".)`. */
private val RUN_NAME = Regex("""^\((?:Scheduled run|Requested run of) "([^"]+)"""")

/**
 * The quiet line a command shows as when no one typed it in this chat, or null for words the owner
 * wrote. A routine's run is named, never shown as its prompt; another agent's message says who.
 */
internal fun chatEvent(command: CommandEnvelope): String? {
    command.origin?.let { origin ->
        return when (origin.kind) {
            AgentMessageKind.Request -> "Message from ${origin.name}"
            AgentMessageKind.Reply -> "${origin.name} replied"
            AgentMessageKind.Failure -> "${origin.name} couldn't answer"
        }
    }
    if (command.surface != AUTOMATIONS_SURFACE) return null
    val text = (command.input as CommandInput.Text).text
    return RUN_NAME.find(text)?.groupValues?.get(1)?.let { "Routine ran · $it" } ?: "A routine ran"
}

package live.renoa.app.agents

import live.renoa.rcp.AgentMessageKind
import live.renoa.rcp.AgentOrigin
import live.renoa.rcp.CommandEnvelope
import live.renoa.rcp.CommandInput

/** The surface a node runs the Host's automation schedule through, as `docs/rcp-v0.md` names it. */
internal const val AUTOMATIONS_SURFACE = "automations"

/** A routine's name in the first line the Host puts on its run: `(Scheduled run "Name", …)` or `(Requested run of "Name".)`. */
private val RUN_NAME = Regex("""^\((?:Scheduled run|Requested run of) "([^"]+)"""")

/** The line over another agent's message or answer, naming it. */
internal fun agentHeading(origin: AgentOrigin): String =
    when (origin.kind) {
        AgentMessageKind.Request -> "Message from ${origin.name}"
        AgentMessageKind.Reply -> "${origin.name} replied"
        AgentMessageKind.Failure -> "${origin.name} couldn't answer"
    }

/**
 * The quiet line a routine's run shows as, named and never shown as its prompt, or null for any
 * other command.
 */
internal fun chatEvent(command: CommandEnvelope): String? {
    if (command.origin != null || command.surface != AUTOMATIONS_SURFACE) return null
    val text = (command.input as CommandInput.Text).text
    return RUN_NAME.find(text)?.groupValues?.get(1)?.let { "Routine ran · $it" } ?: "A routine ran"
}

package live.renoa.app.badge

import androidx.compose.ui.graphics.Color
import live.renoa.app.agents.AgentSummary
import live.renoa.app.theme.RenoaColors

/**
 * The main agent: the one marked [AgentSummary.primary]. Until the Host has said which agent is main,
 * the first on the roster stands in; once it has, no other agent does, even while the one it named
 * is not on the roster. Home seats it in the middle, and it is the one the owner talks to first.
 */
fun mainAgent(roster: List<AgentSummary>): AgentSummary? =
    roster.firstOrNull { it.primary == true } ?: roster.firstOrNull()?.takeIf { roster.all { it.primary == null } }

/**
 * The colour an agent's tile wears. The main agent stays ink; the others take the landing page's
 * clay, cobalt and ochre in roster order, so every screen gives an agent the same colour while the
 * roster keeps its order. Green and purple are never used.
 */
enum class AgentHue {
    Ink,
    Clay,
    Cobalt,
    Ochre,
    ;

    /** The tile's face, or null for ink, which the badge draws in the foreground. */
    fun face(colors: RenoaColors): Color? =
        when (this) {
            Ink -> null
            Clay -> colors.partModel
            Cobalt -> colors.partSkills
            Ochre -> colors.partLoop
        }

    companion object {
        private val PIGMENTS = listOf(Clay, Cobalt, Ochre)

        fun of(agentId: String, roster: List<AgentSummary>): AgentHue {
            val main = mainAgent(roster)?.id
            if (agentId == main) return Ink
            val at = roster.filter { it.id != main }.indexOfFirst { it.id == agentId }
            return if (at < 0) Ink else PIGMENTS[at % PIGMENTS.size]
        }
    }
}

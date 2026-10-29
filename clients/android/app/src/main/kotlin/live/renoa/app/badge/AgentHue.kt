package live.renoa.app.badge

import androidx.compose.ui.graphics.Color
import live.renoa.app.agents.AgentSummary
import live.renoa.app.theme.RenoaColors

/**
 * The main agent: the one marked [AgentSummary.primary], or the first on the roster until one is
 * marked. Home seats it in the middle, and it is the one the owner talks to first.
 */
fun mainAgent(roster: List<AgentSummary>): AgentSummary? = roster.firstOrNull { it.primary } ?: roster.firstOrNull()

/**
 * The colour an agent's tile wears. The main agent stays ink; the others take the landing page's
 * clay, cobalt and ochre in roster order, so each keeps its colour on every screen. Green and
 * purple are never used.
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

package live.renoa.app.badge

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import live.renoa.app.agents.AgentSummary
import live.renoa.app.theme.RenoaColors

/**
 * The main agent: the one marked [AgentSummary.primary], as the Host names it. While none is marked,
 * before the Host has named one or while the one it names is not on this roster, the first on the
 * roster stands in. Home seats it in the middle, and it is the one the owner talks to first.
 */
fun mainAgent(roster: List<AgentSummary>): AgentSummary? = roster.firstOrNull { it.primary } ?: roster.firstOrNull()

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

    /** The faint wash behind the agent's row on Home, or null for ink, which keeps the ground. */
    fun wash(colors: RenoaColors): Color? = face(colors)?.let { lerp(colors.background, it, 0.09f) }

    /** The agent's name in its colour, eased toward the foreground so it reads as text; null for ink. */
    fun name(colors: RenoaColors): Color? = face(colors)?.let { lerp(it, colors.foreground, 0.3f) }

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

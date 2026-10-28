package live.renoa.app.home

import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.Glance
import kotlin.test.Test
import kotlin.test.assertEquals

class HomeOrderTest {
    private fun agent(id: String, state: AgentState, unread: Boolean = false) = AgentSummary(id, id, "", state, Glance(unread = unread))

    @Test
    fun whoeverNeedsTheOwnerComesFirstThenTheStuckTheBusyTheQuietAndTheSleeping() {
        val everyone = AgentState.entries.reversed().map { agent(it.name, it) }
        assertEquals(listOf("Wants", "Stuck", "Working", "Resting", "Asleep"), homeOrder(everyone).map { it.id })
    }

    @Test
    fun withinAStateUnreadComesFirstAndOtherwiseTheRosterOrderStays() {
        val quiet = listOf(agent("a", AgentState.Resting), agent("b", AgentState.Resting, unread = true), agent("c", AgentState.Resting), agent("d", AgentState.Resting, unread = true))
        assertEquals(listOf("b", "d", "a", "c"), homeOrder(quiet).map { it.id })
    }

    @Test
    fun everyStateHasItsOwnPlace() {
        assertEquals(AgentState.entries.size, AgentState.entries.map { it.homeRank }.toSet().size)
    }
}

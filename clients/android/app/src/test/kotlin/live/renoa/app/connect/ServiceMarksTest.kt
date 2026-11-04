package live.renoa.app.connect

import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class ServiceMarksTest {
    /** The label is the link maker's word; only the host the link goes to picks a service's mark. */
    @Test
    fun aMarkComesFromTheHostAlone() {
        assertEquals("google", serviceMark("https://accounts.google.com/o/oauth2/auth")?.name)
        assertEquals("notion", serviceMark("https://mcp.notion.com/authorize")?.name)
        assertNull(serviceMark("https://google.evil.example/authorize"))
        assertNull(serviceMark("https://notgoogle.com/authorize"))
    }

    /** A setup link ends its turn, so the links are read each time a working agent stops. */
    @Test
    fun aTurnEndsWhenAWorkingAgentStops() =
        runBlocking {
            fun roster(vararg working: String) = listOf("a", "b").map { AgentSummary(it, it, "", if (it in working) AgentState.Working else AgentState.Resting) }
            val ended = flowOf(roster(), roster("a"), roster("a", "b"), roster("a", "b"), roster("b"), roster()).turnsEnded().toList()
            assertEquals(2, ended.size)
        }
}

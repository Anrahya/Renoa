package live.renoa.app.home

import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.Ask
import live.renoa.app.agents.Feed
import live.renoa.app.agents.Glance
import live.renoa.app.agents.Note
import live.renoa.app.agents.NoteKind
import java.time.Instant
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class TalksTest {
    private val evening = Instant.parse("2026-10-29T14:10:00Z")

    private fun agent(id: String, state: AgentState = AgentState.Resting, glance: Glance = Glance(), primary: Boolean = false) =
        AgentSummary(id, id.replaceFirstChar { it.uppercase() }, "", state, glance, primary)

    private fun note(id: String, agentId: String?, kind: NoteKind = NoteKind.Update, minutesAgo: Long = 10) =
        Note(id, agentId, kind, if (kind == NoteKind.Problem) "“$id” didn’t finish" else "Wants you to know", "Body of $id.", evening.minusSeconds(minutesAgo * 60))

    private val roster =
        listOf(
            agent("renoa", primary = true),
            agent("sensei", glance = Glance(line = "Today's one thing.")),
            agent("tifa", AgentState.Wants, Glance(ask = Ask("Dinner?", listOf("Dal"), evening.minusSeconds(60)))),
            agent("kaisa", AgentState.Working, Glance(doing = "Finding posts")),
            agent("optimus"),
            agent("ledger", AgentState.Stuck, Glance(line = "Send the statement again?")),
            agent("nagisa", glance = Glance(line = "Atta is on sale.")),
            agent("echo", glance = Glance(line = "Read me.", unread = true)),
        )

    private val feed =
        Feed("Quiet day.", listOf(note("menu", "tifa", minutesAgo = 2), note("projects", "optimus", minutesAgo = 30), note("prices", "nagisa", NoteKind.Problem), note("stray", null)))

    @Test
    fun theMainAgentIsPinnedAndTheOthersSpeakInOrderOfWhoNeedsTheOwnerThenTheNewest() {
        val list = talks(roster, feed)
        assertEquals("renoa", list.main?.agent?.id)
        assertEquals("Quiet day.", list.main?.line, "the main agent with nothing else to say says the day's line")
        assertEquals(listOf("tifa", "ledger", "optimus", "kaisa", "echo"), list.talking.map { it.agent.id })
        assertEquals(listOf("Dinner?", "Send the statement again?", "Body of projects.", "Finding posts…", "Read me."), list.talking.map { it.line })
        assertEquals(listOf("projects"), list.talking.first { it.agent.id == "optimus" }.notes.map { it.id })
        assertEquals(listOf("menu"), list.talking.first().notes.map { it.id }, "a question comes before a newer note, which still goes when the row opens")
        assertEquals(listOf("sensei", "nagisa"), list.quiet.map { it.agent.id }, "the quiet keep Home's order")
        val nagisa = list.quiet.last()
        assertEquals("“prices” didn’t finish", nagisa.line, "a failed routine is a quiet agent's line")
        assertEquals("prices", nagisa.problem?.id)
        assertEquals(emptyList(), nagisa.notes, "a problem is put away by running it again, not by opening the row")
        assertEquals("Today's one thing.", list.quiet.first().line)
    }

    @Test
    fun theMainAgentSaysItsQuestionFirstThenItsNewestNote() {
        val asking = roster.map { if (it.id == "renoa") it.copy(state = AgentState.Wants, glance = Glance(ask = Ask("Gift?", listOf("Show me")))) else it }
        assertEquals("Gift?", talks(asking, feed).main?.line)
        val noted = Feed("Quiet day.", listOf(note("brief", "renoa", minutesAgo = 5), note("older", "renoa", minutesAgo = 50)))
        val main = talks(roster, noted).main
        assertEquals("Body of brief.", main?.line)
        assertEquals(listOf("brief", "older"), main?.notes?.map { it.id })
        assertNull(talks(emptyList(), feed).main)
    }
}

package live.renoa.app.home

import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.Ask
import live.renoa.app.agents.Briefing
import live.renoa.app.agents.Glance
import live.renoa.app.badge.AgentHue
import live.renoa.app.badge.mainAgent
import live.renoa.app.theme.ThemeName
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class HandoverTest {
    private fun agent(id: String, state: AgentState = AgentState.Resting, glance: Glance = Glance(), primary: Boolean = false) = AgentSummary(id, id, "", state, glance, primary)

    @Test
    fun whatNeedsTheOwnerComesFirstThenWhatIsForThemThenWhoIsWorking() {
        val roster =
            listOf(
                agent("lead", AgentState.Working, Glance(doing = "Reading posts", unread = true, line = "Old news")),
                agent("busy", AgentState.Working, Glance(doing = null)),
                agent("news", glance = Glance(line = "Three projects", time = "7:30 pm", unread = true)),
                agent("read", glance = Glance(line = "Seen already")),
                agent("stuck", AgentState.Stuck, Glance(line = "I couldn't open it")),
                agent("asks", AgentState.Wants, Glance(line = "Dinner?", unread = true, ask = Ask("What's for dinner?", listOf("Dal", "Not sure")))),
            )
        val list = handovers(roster, Briefing("lead", "Evening. Two things."))

        assertEquals(
            listOf(
                "asks/needs" to "What's for dinner?",
                "stuck/needs" to "I couldn't open it",
                "lead/note" to "Evening. Two things.",
                "lead/says" to "Old news",
                "news/says" to "Three projects",
                "busy/working" to "Working on it",
            ),
            list.map { it.key to it.text },
        )
        assertEquals(listOf("Dal", "Not sure"), list.first().ask?.choices)
        assertEquals(1, list.count { it.agent.id == "asks" }, "an agent that asks and has unread words has one card, its question")
        assertEquals(listOf("lead/note", "lead/says"), list.filter { it.agent.id == "lead" }.map { it.key }, "the main agent's note is the one card beside its own")
        assertEquals(HandoverKind.Says, list.first { it.key == "news/says" }.kind)
        assertEquals("7:30 pm", list.first { it.key == "news/says" }.time)
        assertEquals(emptyList(), handovers(emptyList(), Briefing("gone", "A note from no one")), "a note from an agent not on the roster is not shown")
    }

    @Test
    fun theWireLightsForTheFirstAgentWorkingWithAnother() {
        val roster =
            listOf(
                agent("alone", AgentState.Working),
                agent("kaisa", AgentState.Working, Glance(with = "renoa")),
                agent("renoa", AgentState.Working, Glance(with = "gone")),
            )
        assertEquals("kaisa" to "renoa", talking(roster)?.let { (a, b) -> a.id to b.id })
        assertNull(talking(listOf(agent("idle", glance = Glance(with = "renoa")), agent("renoa"))), "only an agent at work lights a wire")
    }

    @Test
    fun seatsFollowTheRosterWhileEveryoneFitsAndHomesOrderOnceTheyDoNot() {
        val few = listOf(agent("a"), agent("b"), agent("c", AgentState.Wants))
        assertEquals(
            listOf("a", "b", "c", "new", "", ""),
            seats(few).map { (it as? SeatHolds.Agent)?.agent?.id ?: if (it == SeatHolds.New) "new" else "" },
            "a change of state moves no one",
        )
        val many = List(7) { agent("r$it") } + agent("asks", AgentState.Wants)
        val seated = seats(many)
        assertEquals("asks", (seated.first() as SeatHolds.Agent).agent.id, "whoever needs the owner is seated first")
        assertEquals(SeatHolds.More(3), seated.last())
    }

    @Test
    fun theDockNamesWhoWroteAndCountsTheRest() {
        assertEquals("Chats", dockTitle(emptyList()))
        val fresh = listOf("Tifa", "Optimus", "Nagisa", "Sensei").map { agent(it) }
        assertEquals("New from Tifa", dockTitle(fresh.take(1)))
        assertEquals("New from Tifa and Optimus", dockTitle(fresh.take(2)))
        assertEquals("New from Tifa, Optimus and 2 more", dockTitle(fresh))
    }

    @Test
    fun theMainAgentIsInkAndTheOthersTakeClayCobaltAndOchreInRosterOrder() {
        val roster = listOf(agent("a"), agent("lead", primary = true), agent("b"), agent("c"), agent("d"))
        assertEquals("lead", mainAgent(roster)?.id)
        assertEquals(listOf(AgentHue.Clay, AgentHue.Ink, AgentHue.Cobalt, AgentHue.Ochre, AgentHue.Clay), roster.map { AgentHue.of(it.id, roster) })
        assertEquals("a", mainAgent(listOf(agent("a"), agent("b")))?.id, "until one is marked, the first on the roster is the main agent")
        assertEquals(AgentHue.Ink, AgentHue.of("stranger", roster))

        for (theme in ThemeName.entries) {
            val c = theme.colors
            assertNull(AgentHue.Ink.face(c), "the main agent is drawn in the foreground")
            assertEquals(listOf(c.partModel, c.partSkills, c.partLoop), listOf(AgentHue.Clay, AgentHue.Cobalt, AgentHue.Ochre).map { it.face(c) }, "never the green or purple parts")
        }
    }
}

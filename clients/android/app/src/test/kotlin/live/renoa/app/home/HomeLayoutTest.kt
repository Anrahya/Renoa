package live.renoa.app.home

import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class HomeLayoutTest {
    private fun agent(name: String, state: AgentState) = AgentSummary(name.lowercase(), name, "", state)

    @Test
    fun theRankOrdersEveryBusyStateBeforeEveryQuietOneAndGivesNoTwoStatesTheSameRank() {
        val ranks = AgentState.entries.map { it.stageRank }
        assertEquals(ranks.size, ranks.toSet().size, "two states share a rank")
        val worstBusy = AgentState.entries.filter { it.onStage }.maxOf { it.stageRank }
        val bestQuiet = AgentState.entries.filter { !it.onStage }.minOf { it.stageRank }
        assertTrue(worstBusy < bestQuiet, "a quiet state outranks a busy one")
        assertEquals(setOf(AgentState.Wants, AgentState.Stuck, AgentState.Working), AgentState.entries.filter { it.onStage }.toSet())
    }

    @Test
    fun theAgentsWhoNeedTheOwnerTakeTheStageBeforeTheOnesJustWorking() {
        val layout =
            HomeLayout.of(
                listOf(
                    agent("Renoa", AgentState.Working),
                    agent("Kai", AgentState.Working),
                    agent("Optimus", AgentState.Wants),
                    agent("Ledger", AgentState.Stuck),
                    agent("Mochi", AgentState.Working),
                ),
                hideAsleep = false,
            )
        assertEquals(listOf("Optimus", "Ledger", "Renoa"), layout.stage.map { it.name })
        assertEquals(listOf("Kai", "Mochi"), layout.hive.map { it.name }, "busy agents that did not fit go to the hive")
    }

    @Test
    fun aQuietAgentNeverTakesTheStageEvenWhenItIsEmpty() {
        val layout = HomeLayout.of(listOf(agent("Tifa", AgentState.Resting), agent("Nova", AgentState.Asleep)), hideAsleep = false)
        assertEquals(emptyList(), layout.stage)
        assertEquals(listOf("Tifa", "Nova"), layout.hive.map { it.name })
    }

    @Test
    fun theHiveShowsAwakeAgentsBeforeSleepingOnesAndKeepsTheirOrder() {
        val layout =
            HomeLayout.of(
                listOf(
                    agent("Nova", AgentState.Asleep),
                    agent("Tifa", AgentState.Resting),
                    agent("Miso", AgentState.Asleep),
                    agent("Atlas", AgentState.Resting),
                ),
                hideAsleep = false,
            )
        assertEquals(listOf("Tifa", "Atlas", "Nova", "Miso"), layout.hive.map { it.name })
        assertEquals(2, layout.asleepCount)
    }

    @Test
    fun tidyingUpHidesTheSleepersButStillCountsThem() {
        val agents = listOf(agent("Tifa", AgentState.Resting), agent("Nova", AgentState.Asleep), agent("Miso", AgentState.Asleep))
        val tidy = HomeLayout.of(agents, hideAsleep = true)
        assertEquals(listOf("Tifa"), tidy.hive.map { it.name })
        assertEquals(2, tidy.asleepCount)
        assertTrue(tidy.asleepHidden)
    }

    @Test
    fun everyAgentAppearsExactlyOnceWhereverItGoes() {
        val agents = AgentState.entries.flatMap { state -> (1..4).map { agent("${state.name}$it", state) } }
        val layout = HomeLayout.of(agents, hideAsleep = false)
        val placed = layout.stage + layout.hive
        assertEquals(agents.map { it.id }.sorted(), placed.map { it.id }.sorted())
        assertEquals(HomeLayout.STAGE_SLOTS, layout.stage.size)
    }

    @Test
    fun theStageHasASlotForEachAgentAndTheFirstIsTheLargest() {
        for (count in 0..HomeLayout.STAGE_SLOTS) {
            val slots = HomeLayout.slots(count)
            assertEquals(count, slots.size)
            if (count > 0) assertEquals(slots.maxOf { it.size }, slots.first().size)
            assertTrue(slots.all { it.left >= 0f && it.left + it.size <= 390f }, "$count agents: a slot leaves the 390-wide design")
            assertTrue(slots.all { it.top + it.size + 24f <= HomeLayout.stageHeight(count) + 1f }, "$count agents: a slot and its name leave the stage")
        }
    }
}

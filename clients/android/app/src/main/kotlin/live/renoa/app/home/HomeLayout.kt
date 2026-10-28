package live.renoa.app.home

import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary

/** Where one agent stands on Home's stage, in the design's 390-wide units. */
data class StageSlot(val left: Float, val top: Float, val size: Float)

/**
 * Who is on the busy stage and who is in the hive. The rule has one owner,
 * [AgentState.onStage] and [AgentState.stageRank]: busy agents take the stage, most urgent
 * first, up to [STAGE_SLOTS]; everyone else, and any busy agent that did not fit, sits in the hive,
 * awake before asleep.
 */
data class HomeLayout(
    val stage: List<AgentSummary>,
    val hive: List<AgentSummary>,
    /** How many agents are asleep, whether or not the hive is showing them. */
    val asleepCount: Int,
    val asleepHidden: Boolean,
) {
    companion object {
        const val STAGE_SLOTS = 3
        val Empty = HomeLayout(emptyList(), emptyList(), 0, false)

        fun of(agents: List<AgentSummary>, hideAsleep: Boolean): HomeLayout {
            val busy = agents.filter { it.state.onStage }.sortedBy { it.state.stageRank }
            val stage = busy.take(STAGE_SLOTS)
            val stageIds = stage.map { it.id }.toSet()
            val rest = agents.filter { it.id !in stageIds }
            val awake = rest.filter { it.state != AgentState.Asleep }
            val asleep = rest.filter { it.state == AgentState.Asleep }
            val asleepTotal = agents.count { it.state == AgentState.Asleep }
            return HomeLayout(stage, awake + if (hideAsleep) emptyList() else asleep, asleepTotal, hideAsleep)
        }

        /** Slot positions for [count] stage agents: the first is the largest. One agent stands alone in the middle. */
        fun slots(count: Int): List<StageSlot> =
            when (count) {
                0 -> emptyList()
                1 -> listOf(StageSlot(120f, 21f, 150f))
                2 -> listOf(StageSlot(33f, 21f, 150f), StageSlot(233f, 14f, 118f))
                else -> listOf(StageSlot(33f, 21f, 150f), StageSlot(233f, 14f, 118f), StageSlot(196f, 168f, 104f))
            }

        /** Height of the stage in design units: the third slot hangs lower than the first two. */
        fun stageHeight(count: Int): Float =
            when (count) {
                0 -> 0f
                1, 2 -> 200f
                else -> 300f
            }
    }
}

package live.renoa.app.badge

import live.renoa.app.agents.AgentState
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class BadgeMotionTest {
    private val times = listOf(0f, 0.2f, 0.7f, 1.3f, 2.9f, 3.0f, 5.5f, 17.1f)

    private fun frames(state: AgentState, mini: Boolean = false, reduced: Boolean = false) =
        times.map { BadgeMotion.frame(state, mini, it, reduced) }

    @Test
    fun everyStateHasOneMotionAndOnlyTheMovingOnesNeedAClock() {
        val moving = AgentState.entries.filter { BadgeMotion.animates(it, reducedMotion = false) }.toSet()
        assertEquals(setOf(AgentState.Resting, AgentState.Working, AgentState.Wants), moving)
        for (state in AgentState.entries) {
            val distinct = frames(state).toSet().size
            if (state in moving) assertTrue(distinct > 1, "$state should change over time") else assertEquals(1, distinct, "$state should be still")
        }
    }

    @Test
    fun withReducedMotionNothingLoopsInAnyState() {
        for (state in AgentState.entries) {
            assertFalse(BadgeMotion.animates(state, reducedMotion = true), "$state")
            assertEquals(1, frames(state, reduced = true).toSet().size, "$state must be the same at every time")
            assertEquals(1, frames(state, mini = true, reduced = true).toSet().size, "mini $state must be the same at every time")
        }
    }

    @Test
    fun withReducedMotionTheFiveStatesStillLookDifferent() {
        val still = AgentState.entries.associateWith { BadgeMotion.frame(it, false, 0f, true) }
        for (a in AgentState.entries) for (b in AgentState.entries) {
            if (a.ordinal < b.ordinal) assertNotEquals(still.getValue(a), still.getValue(b), "$a and $b are indistinguishable when still")
        }
    }

    @Test
    fun restingBreathesBetweenFullSizeAndThreePercentLarger() {
        val scales = (0..600).map { BadgeMotion.frame(AgentState.Resting, false, it / 100f, false).coreScale }
        assertTrue(scales.all { it in 1f..1.0301f }, "scale left its range: ${scales.min()}..${scales.max()}")
        assertEquals(1.03f, BadgeMotion.frame(AgentState.Resting, false, 3f, false).coreScale, 1e-4f)
        assertEquals(1f, BadgeMotion.frame(AgentState.Resting, false, 0f, false).coreScale, 1e-4f)
    }

    @Test
    fun workingLightsTheSixTilesOneAfterAnotherAroundTheCore() {
        // After the first cycle every tile is on its own beat: tile k peaks 0.27s after its delay of 0.3k seconds.
        val cycle = 3 * 1.8f
        for (k in 0 until 6) {
            val frame = BadgeMotion.frame(AgentState.Working, false, cycle + 0.3f * k + 0.27f, false)
            assertEquals(1f, frame.orbs[k].alpha, 1e-3f, "tile $k should be at its brightest")
            for (other in frame.orbs.indices.filter { it != k }) {
                assertTrue(frame.orbs[other].alpha < 0.999f, "tile $other should not peak with tile $k")
            }
        }
    }

    @Test
    fun aWorkingTileSwellsAsItLightsAndShrinksBackWhenItDims() {
        val peak = BadgeMotion.frame(AgentState.Working, false, 3.6f + 0.27f, false).orbs[0]
        val rest = BadgeMotion.frame(AgentState.Working, false, 3.6f + 1.2f, false).orbs[0]
        assertEquals(1.14f, peak.scale, 1e-3f)
        assertEquals(0.88f, rest.scale, 1e-3f)
        assertEquals(0.35f, rest.alpha, 1e-3f)
    }

    @Test
    fun workingGlowsInItsOwnColourBetweenAQuarterAndSixTenths() {
        val glows = (0..400).map { BadgeMotion.frame(AgentState.Working, false, it / 100f, false).glowAlpha }
        assertTrue(glows.all { it in 0.2499f..0.6001f })
        assertEquals(0.6f, BadgeMotion.frame(AgentState.Working, false, 0.9f, false).glowAlpha, 1e-3f)
    }

    @Test
    fun wantsRisesAndHoversWithOneDotAndAShadow() {
        val lifts = (0..640).map { BadgeMotion.frame(AgentState.Wants, false, it / 100f, false).lift }
        assertTrue(lifts.all { it in -13.001f..-8.999f }, "lift ${lifts.min()}..${lifts.max()}")
        assertTrue(lifts.min() < -12.9f && lifts.max() > -9.1f, "it should travel the whole range")
        frames(AgentState.Wants).forEach {
            assertEquals(Dot.Calling, it.dot)
            assertEquals(0.5f, it.shadowAlpha)
        }
    }

    @Test
    fun stuckLosesItsColourAndShowsOneRedDot() {
        frames(AgentState.Stuck).forEach {
            assertTrue(it.grayscale)
            assertEquals(Dot.Failed, it.dot)
            assertEquals(0.8f, it.brightness)
            assertEquals(1f, it.alpha)
        }
    }

    @Test
    fun asleepIsGreyAndFaintWithNoDot() {
        frames(AgentState.Asleep).forEach {
            assertTrue(it.grayscale)
            assertEquals(0.38f, it.alpha)
            assertNull(it.dot)
        }
    }

    @Test
    fun aSmallBadgeHasNoTilesAndSaysWorkingWithAPulsingDot() {
        for (state in AgentState.entries) {
            frames(state, mini = true).forEach { assertTrue(it.orbs.isEmpty(), "$state") }
        }
        val scales = frames(AgentState.Working, mini = true).map { it.dotScale }
        assertTrue(scales.toSet().size > 1, "the dot should pulse")
        frames(AgentState.Working, mini = true).forEach { assertEquals(Dot.Running, it.dot) }
        frames(AgentState.Working, mini = false).forEach { assertNull(it.dot, "a full badge says working with its tiles, not a dot") }
    }

    @Test
    fun aTileWaitsAtFullSizeUntilItsTurnComesRound() {
        val start = BadgeMotion.frame(AgentState.Working, false, 0f, false)
        assertEquals(OrbFrame(1f, 1f), start.orbs[5])
    }

    private fun assertFalse(value: Boolean, message: String) = assertTrue(!value, message)
}

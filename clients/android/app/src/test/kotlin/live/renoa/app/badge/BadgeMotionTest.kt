package live.renoa.app.badge

import live.renoa.app.agents.AgentState
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class BadgeMotionTest {
    private fun frames(state: AgentState, reduced: Boolean = false) = (0..400).map { BadgeMotion.frame(state, it * 0.02f, reduced) }

    @Test
    fun onlyWorkingAndWantsYouMoveSoOnlyTheyNeedAClock() {
        for (state in AgentState.entries) {
            val moves = frames(state).toSet().size > 1
            assertEquals(moves, BadgeMotion.animates(state, reducedMotion = false), "$state")
            assertTrue(!BadgeMotion.animates(state, reducedMotion = true))
        }
    }

    @Test
    fun withReducedMotionNothingLoopsAndTheFiveStatesStillLookDifferent() {
        for (state in AgentState.entries) assertEquals(1, frames(state, reduced = true).toSet().size, "$state")
        assertEquals(AgentState.entries.size, AgentState.entries.map { BadgeMotion.frame(it, 0f, true) }.toSet().size)
    }

    @Test
    fun workingSendsOutARingAndShowsTheRunningDot() {
        val rings = frames(AgentState.Working).map { it.ring!! }
        assertTrue(rings.all { it in 0f..1f })
        assertTrue(rings.min() < 0.1f && rings.max() > 0.9f, "a ring leaves the tile and fades out")
        assertTrue(frames(AgentState.Working).all { it.dot == Dot.Running && it.lift == 0f })
    }

    @Test
    fun wantsYouLiftsOffItsSocketWithTheCallingDot() {
        val wants = frames(AgentState.Wants)
        assertTrue(wants.all { it.lift in 0.6f..1f && it.dot == Dot.Calling })
        assertNull(frames(AgentState.Wants).first().ring)
    }

    @Test
    fun stuckTurnsItsEdgeRedAndAsleepRetires() {
        val stuck = BadgeMotion.frame(AgentState.Stuck, 0f, false)
        assertTrue(stuck.failed && stuck.dot == Dot.Failed)
        val asleep = BadgeMotion.frame(AgentState.Asleep, 0f, false)
        assertTrue(asleep.retiring && asleep.dot == null && asleep.alpha < 1f)
        assertEquals(BadgeFrame(), BadgeMotion.frame(AgentState.Resting, 3f, false), "resting sits still on its socket")
    }

    @Test
    fun theLetterIsTheFirstLetterOrDigitOrAQuestionMark() {
        assertEquals('K', tileLetter("  kai'sa"))
        assertEquals('?', tileLetter(""))
        assertEquals('?', tileLetter("  !!"))
        assertEquals('7', tileLetter("7even"))
    }

    @Test
    fun theHaloLightIsBrightestAtItsHeadAndFadesBehindIt() {
        assertEquals(1f, haloLight(0.5f, 0.5f), 1e-4f)
        assertTrue(haloLight(0.45f, 0.5f) in 0.5f..0.9f)
        assertEquals(0f, haloLight(0.55f, 0.5f), "nothing ahead of the head")
        assertTrue(haloLight(0.95f, 0.02f) > 0.5f, "the tail wraps past the top")
    }
}

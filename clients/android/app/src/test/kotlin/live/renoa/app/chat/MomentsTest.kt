package live.renoa.app.chat

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class MomentsTest {
    @Test
    fun theWorkingLineShowsOneWordAtATimeAndWrapsRound() {
        val shown = (0 until 40).map { WorkingWords.frame(4, it * 0.25f + 0.5f, reducedMotion = false).index }
        assertEquals(listOf(0, 1, 2, 3), shown.distinct().take(4))
        assertEquals(0, WorkingWords.frame(4, 4 * WorkingWords.SECONDS_EACH + 1f, reducedMotion = false).index, "after the last word comes the first")
    }

    @Test
    fun aWordFadesInHoldsAndFadesOutAlwaysWithinZeroAndOne() {
        val alphas = (0..1000).map { WorkingWords.frame(4, it * 0.01f, reducedMotion = false).alpha }
        assertTrue(alphas.all { it in 0f..1f })
        assertEquals(0f, WorkingWords.frame(4, 0f, false).alpha, 1e-4f)
        assertEquals(1f, WorkingWords.frame(4, 1.25f, false).alpha, 1e-4f)
        assertTrue(WorkingWords.frame(4, 2.45f, false).alpha < 0.2f, "the word is almost gone at the end of its turn")
    }

    @Test
    fun aWordDriftsInFromBelowAndOutAbove() {
        assertTrue(WorkingWords.frame(4, 0f, false).rise > 0f)
        assertEquals(0f, WorkingWords.frame(4, 1.25f, false).rise)
        assertTrue(WorkingWords.frame(4, 2.45f, false).rise < 0f)
    }

    @Test
    fun withReducedMotionTheFirstWordStaysStill() {
        for (t in listOf(0f, 3f, 7.7f, 100f)) assertEquals(WordFrame(0, 1f, 0f), WorkingWords.frame(4, t, reducedMotion = true))
    }

    @Test
    fun thereMustBeAWordToShow() {
        assertFailsWith<IllegalArgumentException> { WorkingWords.frame(0, 0f, false) }
    }

    @Test
    fun aNewWordSharpensIntoPlaceOverAboutHalfASecondAndAnOldOneIsFullyThere() {
        val arrivals = listOf(0f, 1f)
        assertEquals(1f, wordAlpha(arrivals, 0, 5f))
        assertEquals(0f, wordAlpha(arrivals, 1, 1f))
        assertTrue(wordAlpha(arrivals, 1, 1.2f) in 0.1f..0.9f)
        assertEquals(1f, wordAlpha(arrivals, 1, 1.5f))
        assertEquals(0f, wordAlpha(arrivals, 1, 0.5f), "a word cannot be visible before it arrives")
    }
}

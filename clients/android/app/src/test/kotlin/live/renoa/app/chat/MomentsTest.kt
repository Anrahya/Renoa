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

    /** Runs the reveal at 60 frames a second; [arrivals] says how many characters have arrived by each frame. */
    private fun reveal(frames: Int, complete: (Int) -> Boolean = { false }, arrivals: (Int) -> Int): List<Float> {
        var progress = Reveal.Progress()
        return (0 until frames).map { frame ->
            progress = Reveal.step(progress, arrivals(frame), 1f / 60, complete(frame))
            progress.shown
        }
    }

    @Test
    fun aBurstIsShownWithinTheLagLimitNotAtTheBasePace() {
        val shown = reveal(120) { 300 }
        val frames = shown.indexOfFirst { it >= 300f } + 1
        assertTrue(frames / 60f <= Reveal.MAX_LAG_SECONDS + 1f / 60, "300 characters took ${frames / 60f} s")
    }

    @Test
    fun everyCharacterShowsWithinTheLagLimitOfArrivingEvenWhenWordsKeepComing() {
        // A burst, then a word every 60 ms: the later small words must not slow the reveal of the burst.
        val arrivals = { frame: Int -> 200 + 6 * (frame / 4) }
        val shown = reveal(240, arrivals = arrivals)
        val lagFrames = (Reveal.MAX_LAG_SECONDS * 60).toInt() + 1
        for (frame in lagFrames until shown.size) {
            assertTrue(shown[frame] >= arrivals(frame - lagFrames), "at frame $frame, ${shown[frame]} shown of ${arrivals(frame - lagFrames)} due")
        }
    }

    @Test
    fun aTrickleIsShownAtTheBasePaceAndNeverPastWhatHasArrived() {
        assertEquals(Reveal.BASE_CHARS_PER_SECOND / 60, Reveal.step(Reveal.Progress(), 20, 1f / 60, complete = false).shown, 1e-4f)
        assertEquals(5f, Reveal.step(Reveal.Progress(4.9f), 5, 1f, complete = false).shown)
    }

    @Test
    fun aWholeReplyFinishesItsRevealQuickly() {
        val shown = reveal(120, complete = { true }) { 600 }
        val frames = shown.indexOfFirst { it >= 600f } + 1
        assertTrue(frames / 60f <= Reveal.FINISH_SECONDS + 1f / 60, "a finished reply took ${frames / 60f} s to show")
    }

    @Test
    fun theSpeedResetsOnceTheRevealCatchesUp() {
        assertEquals(Reveal.Progress(300f, 0f), Reveal.step(Reveal.Progress(299f, 1000f), 300, 1f / 60, complete = false), "the step that catches up resets")
        assertEquals(Reveal.BASE_CHARS_PER_SECOND / 60, Reveal.step(Reveal.Progress(300f, 0f), 320, 1f / 60, complete = false).shown - 300f, 1e-3f, "so the next word goes at the base pace")
    }

    @Test
    fun aWholeReplyEndsWithEveryCharacterFullyClearButOneStillArrivingKeepsASoftEdge() {
        val length = 50
        assertTrue((0 until length).all { Reveal.alpha(it, Reveal.end(length, complete = true).toFloat()) == 1f })
        assertTrue(Reveal.alpha(length - 1, Reveal.end(length, complete = false).toFloat()) < 1f)
    }

    @Test
    fun eachCharacterFadesInBehindTheEdge() {
        assertEquals(1f, Reveal.alpha(0, 40f))
        assertEquals(0f, Reveal.alpha(40, 40f), "the edge itself is not shown yet")
        assertTrue(Reveal.alpha(33, 40f) in 0.1f..0.9f)
        assertEquals(0f, Reveal.alpha(50, 40f), "nothing past the edge shows")
    }
}

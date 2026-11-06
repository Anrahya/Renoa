package live.renoa.app.home

import live.renoa.app.agents.LastRun
import live.renoa.app.agents.Routine
import live.renoa.app.agents.RunOutcome
import java.time.LocalTime
import java.time.ZoneId
import java.time.ZonedDateTime
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class TodayTest {
    private val zone = ZoneId.of("Asia/Kolkata")

    /** Thursday 29 October 2026, 7:40 PM. */
    private val now = ZonedDateTime.of(2026, 10, 29, 19, 40, 0, 0, zone)

    private fun at(days: Long, hour: Int, minute: Int = 0) = now.plusDays(days).with(LocalTime.of(hour, minute)).toInstant()

    private fun routine(id: String, next: Int? = null, nextDays: Long = 0, ran: Int? = null, ranDays: Long = 0, outcome: RunOutcome = RunOutcome.Done, on: Boolean = true) =
        Routine(id, null, id, "Every day", on, next?.let { at(nextDays, it) }, ran?.let { LastRun(at(ranDays, it), outcome) })

    @Test
    fun nextRunKeepsOnlyASwitchedOnRunStillToCome() {
        assertNull(nextRun(routine("off", next = 21, on = false), now))
        assertNull(nextRun(routine("past", next = 18), now))
        assertEquals(now, nextRun(Routine("due", null, "due", "Every day", true, now.toInstant(), null), now))
        assertEquals(21, nextRun(routine("soonest", next = 21), now)?.hour)
        assertEquals(Horizon.Today, Horizon.of(now.withHour(22), now))
        assertEquals(Horizon.Tomorrow, Horizon.of(now.plusDays(1).withHour(7), now))
        assertEquals(Horizon.ThisWeek, Horizon.of(now.plusDays(6).withHour(10), now))
        assertEquals(Horizon.Later, Horizon.of(now.plusDays(7).withHour(10), now))
    }

    @Test
    fun aRoutineSaysWhetherItIsPausedWhenItRunsAndWhetherItFailed() {
        assertEquals("Paused", routineStatus(routine("a", on = false), now))
        assertEquals("Due now", routineStatus(routine("a", next = 19), now), "Renoa has not yet said when it runs after this one")
        assertEquals("Due now", routineStatus(routine("a", next = 9, nextDays = -1), now))
        assertEquals("Next: today at 9 PM", routineStatus(routine("a", next = 21), now))
        assertEquals("Next: tomorrow at 7 AM · Last run failed", routineStatus(routine("a", next = 7, nextDays = 1, ran = 7, outcome = RunOutcome.Failed), now))
        assertEquals("Next: Sat at 10 AM", routineStatus(routine("a", next = 10, nextDays = 2), now))
        assertEquals("Next: Wed at 10 AM", routineStatus(routine("a", next = 10, nextDays = 6), now))
        assertEquals("Next: 5 Nov at 10 AM", routineStatus(routine("a", next = 10, nextDays = 7), now), "a week off is named by its date, not a weekday")
        assertEquals("Next: 9 Nov at 10 AM", routineStatus(routine("a", next = 10, nextDays = 11), now))
    }

    @Test
    fun routinesListSoonestFirstOverdueBeforeThatAndPausedLast() {
        val routines =
            listOf(
                routine("off", on = false, next = 8),
                routine("monthly", next = 9, nextDays = 11),
                routine("brief", next = 7, nextDays = 1),
                routine("done", next = null),
                routine("overdue", next = 9, nextDays = -1),
                routine("posts", next = 20),
            )
        assertEquals(listOf("overdue", "posts", "brief", "monthly", "off", "done"), soonestFirst(routines).map { it.id })
    }
}

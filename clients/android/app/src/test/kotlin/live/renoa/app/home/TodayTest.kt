package live.renoa.app.home

import live.renoa.app.agents.LastRun
import live.renoa.app.agents.Routine
import live.renoa.app.agents.RunOutcome
import java.time.LocalTime
import java.time.ZoneId
import java.time.ZonedDateTime
import kotlin.test.Test
import kotlin.test.assertEquals

class TodayTest {
    private val zone = ZoneId.of("Asia/Kolkata")

    /** Thursday 29 October 2026, 7:40 PM. */
    private val now = ZonedDateTime.of(2026, 10, 29, 19, 40, 0, 0, zone)

    private fun at(days: Long, hour: Int, minute: Int = 0) = now.plusDays(days).with(LocalTime.of(hour, minute)).toInstant()

    private fun routine(id: String, next: Int? = null, nextDays: Long = 0, ran: Int? = null, ranDays: Long = 0, outcome: RunOutcome = RunOutcome.Done, on: Boolean = true) =
        Routine(id, null, id, "Every day", on, next?.let { at(nextDays, it) }, ran?.let { LastRun(at(ranDays, it), outcome) })

    @Test
    fun theGreetingFollowsTheHour() {
        assertEquals(
            listOf("Good night.", "Good morning.", "Good morning.", "Good afternoon.", "Good afternoon.", "Good evening.", "Good evening.", "Good night."),
            listOf(4, 5, 11, 12, 16, 17, 21, 22).map { greeting(it) },
        )
        assertEquals("Thu 7:40 PM", clockWords(now))
    }

    @Test
    fun theStripRunsFromSixInTheMorningToMidnight() {
        assertEquals(0f, stripAt(LocalTime.of(6, 0)))
        assertEquals(0f, stripAt(LocalTime.of(2, 0)), "earlier hours sit at the start")
        assertEquals(1f / 3, stripAt(LocalTime.NOON), 0.0001f)
        assertEquals(0.75f, stripAt(LocalTime.of(19, 30)), 0.0001f)
        assertEquals(1f, stripAt(LocalTime.MAX), 0.0001f)
    }

    @Test
    fun todaysRunsAndTodaysNextRunsAreMarkedInTimeOrder() {
        val routines =
            listOf(
                routine("checkin", next = 21, ran = 21, ranDays = -1),
                routine("brief", next = 7, nextDays = 1, ran = 7),
                routine("prices", next = 19, nextDays = 1, ran = 19, outcome = RunOutcome.Failed),
                routine("lunch", on = false),
            )
        assertEquals(
            listOf(Triple("brief", 7, RunOutcome.Done), Triple("prices", 19, RunOutcome.Failed), Triple("checkin", 21, null)),
            dayMarks(routines, now).map { Triple(it.routine.id, it.at.hour, it.ran) },
            "yesterday's run and tomorrow's runs are not today's",
        )
        assertEquals("Next: checkin at 9 PM", nextWords(routines, now))
        assertEquals("Nothing else is set for today", nextWords(routines.drop(1), now))
        assertEquals("Nothing else is set for today", nextWords(listOf(routine("late", next = 19)), now), "a next run already gone by is not next")
    }

    @Test
    fun aRoutineSaysWhetherItIsPausedWhenItRunsAndWhetherItFailed() {
        assertEquals("Paused", routineStatus(routine("a", on = false), now))
        assertEquals("Next: today at 9 PM", routineStatus(routine("a", next = 21), now))
        assertEquals("Next: tomorrow at 7 AM · Last run failed", routineStatus(routine("a", next = 7, nextDays = 1, ran = 7, outcome = RunOutcome.Failed), now))
        assertEquals("Next: Sat at 10 AM", routineStatus(routine("a", next = 10, nextDays = 2), now))
        assertEquals("Next: Wed at 10 AM", routineStatus(routine("a", next = 10, nextDays = 6), now))
        assertEquals("Next: 5 Nov at 10 AM", routineStatus(routine("a", next = 10, nextDays = 7), now), "a week off is named by its date, not a weekday")
        assertEquals("Next: 9 Nov at 10 AM", routineStatus(routine("a", next = 10, nextDays = 11), now))
    }

    @Test
    fun upcomingGroupsEachNextRunByHowFarOffItIs() {
        val routines =
            listOf(
                routine("monthly", next = 9, nextDays = 11),
                routine("brief", next = 7, nextDays = 1),
                routine("posts", next = 20),
                routine("weekly", next = 10, nextDays = 3),
                routine("gone", next = 19),
                routine("off", on = false),
            )
        assertEquals(
            listOf(
                "Later today" to listOf("posts" to "8 PM"),
                "Tomorrow" to listOf("brief" to "7 AM"),
                "This week" to listOf("weekly" to "Sun 10 AM"),
                "Later" to listOf("monthly" to "9 Nov"),
            ),
            upcoming(routines, now).map { group -> group.horizon.heading to group.runs.map { (r, time) -> r.id to time } },
        )
    }
}

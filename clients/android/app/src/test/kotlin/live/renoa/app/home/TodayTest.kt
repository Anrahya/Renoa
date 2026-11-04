package live.renoa.app.home

import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.Ask
import live.renoa.app.agents.Feed
import live.renoa.app.agents.Glance
import live.renoa.app.agents.LastRun
import live.renoa.app.agents.Note
import live.renoa.app.agents.NoteKind
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
    fun theStripRunsFromSixToMidnightWithTheHourEitherSideOfNowDrawnWide() {
        val lens = DayLens(LocalTime.of(19, 40))
        assertEquals(0f, lens.at(LocalTime.of(6, 0)))
        assertEquals(0f, lens.at(LocalTime.of(2, 0)), "earlier hours sit at the start")
        assertEquals(1f, lens.at(LocalTime.MAX), 0.0001f)
        // 6:40 to 8:40 PM take 0.36 of the width, and the other sixteen hours share the rest.
        assertEquals(0.24f, lens.at(LocalTime.NOON), 0.0001f)
        assertEquals(0.04f, lens.at(LocalTime.of(13, 0)) - lens.at(LocalTime.NOON), 0.0001f)
        assertEquals(0.18f, lens.at(LocalTime.of(20, 40)) - lens.at(LocalTime.of(19, 40)), 0.0001f)
        for (hour in listOf(6f, 9.5f, 18.7f, 19.2f, 20.6f, 22f, 24f)) assertEquals(hour, lens.hourAt(lens.atHour(hour)), 0.001f, "$hour and back")

        val early = DayLens(LocalTime.of(3, 0))
        assertEquals(0.36f, early.at(LocalTime.of(7, 0)), 0.0001f, "before six, the first hour is the one drawn wide")
        assertEquals(1f, DayLens(LocalTime.of(23, 50)).at(LocalTime.MAX), 0.0001f)
    }

    @Test
    fun theStripMarksQuestionsNotesAndRunsOfTodayOnly() {
        val tifa = AgentSummary("tifa", "Tifa", "", AgentState.Wants, Glance(ask = Ask("Dinner?", listOf("Dal"), at(0, 19, 30))))
        val old = AgentSummary("old", "Old", "", AgentState.Wants, Glance(ask = Ask("Still?", emptyList(), at(-1, 19, 30))))
        val feed =
            Feed(
                "Quiet day.",
                listOf(
                    Note("n", "optimus", NoteKind.Update, "Wants you to know", "Three projects.", at(0, 18, 10)),
                    Note("p", "nagisa", NoteKind.Problem, "\u201cPrices\u201d didn\u2019t finish", "Run it again.", at(0, 12, 30)),
                    Note("y", "optimus", NoteKind.Update, "Wants you to know", "Yesterday.", at(-1, 18, 10)),
                ),
            )
        val roster = listOf(tifa, old, AgentSummary("optimus", "Optimus", "", AgentState.Resting))
        val marks = stripMarks(roster, feed, listOf(routine("brief", ran = 7), routine("checkin", next = 21)), now)
        assertEquals(
            listOf(
                MarkKind.Ran to "brief ran at 7 AM",
                MarkKind.Problem to "\u201cPrices\u201d didn\u2019t finish",
                MarkKind.Note to "Optimus: Three projects.",
                MarkKind.Ask to "Tifa asks: Dinner?",
                MarkKind.Coming to "checkin at 9 PM",
            ),
            marks.map { it.kind to it.words },
            "yesterday's question and note are not today's",
        )
        assertEquals("Next: checkin at 9 PM \u00b7 1 ran", stripLine(marks, listOf(routine("brief", ran = 7), routine("checkin", next = 21)), now))
        val failed = listOf(routine("prices", ran = 19, outcome = RunOutcome.Failed))
        assertEquals("Nothing else is set for today \u00b7 1 didn\u2019t finish", stripLine(stripMarks(emptyList(), Feed(), failed, now), failed, now))

        val lens = DayLens(now.toLocalTime())
        val ask = lens.at(LocalTime.of(19, 30))
        assertEquals("tifa", nearest(marks, lens, ask + 0.01f, within = 0.02f)?.agentId)
        assertNull(nearest(marks, lens, ask + 0.01f, within = 0.005f), "too far from any mark names none")
    }

    @Test
    fun marksTooCloseToTellApartShowAsOne() {
        assertEquals(listOf(listOf(0f, 5f), listOf(20f, 26f), listOf(40f)), clustered(listOf(40f, 0f, 26f, 5f, 20f), { it }, within = 8f))
        assertEquals(listOf(listOf(0f, 7f, 14f)), clustered(listOf(0f, 7f, 14f), { it }, within = 8f), "each close to the next joins one run")
        assertEquals(listOf(listOf(0f), listOf(8f)), clustered(listOf(0f, 8f), { it }, within = 8f))
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
        val late = routine("late", next = 19, ran = 19, ranDays = -1)
        assertEquals("Nothing else is set for today", nextWords(listOf(late), now), "a next run already gone by is not next")
        assertEquals(emptyList(), dayMarks(listOf(late), now), "nor is it still to come on the strip")
        assertEquals(RunOutcome.Skipped, dayMarks(listOf(routine("s", ran = 8, outcome = RunOutcome.Skipped)), now).single().ran)
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

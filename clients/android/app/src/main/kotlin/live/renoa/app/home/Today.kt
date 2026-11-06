package live.renoa.app.home

import live.renoa.app.agents.Routine
import live.renoa.app.agents.RunOutcome
import live.renoa.app.agents.clockTime
import java.time.ZonedDateTime
import java.time.format.DateTimeFormatter
import java.time.temporal.ChronoUnit
import java.util.Locale

private val WEEKDAY = DateTimeFormatter.ofPattern("EEE", Locale.ENGLISH)
private val DATE = DateTimeFormatter.ofPattern("d MMM", Locale.ENGLISH)

/**
 * When [routine] runs next, if that is still to come. Renoa updates a routine's next run on its own
 * clock, so for a moment after a run the next one it last said can already be past.
 */
fun nextRun(routine: Routine, now: ZonedDateTime): ZonedDateTime? = routine.next?.takeIf { routine.on }?.atZone(now.zone)?.takeIf { !it.isBefore(now) }

/** How far off a day is, which names it in a time. */
enum class Horizon {
    Today,
    Tomorrow,
    ThisWeek,
    Later,
    ;

    companion object {
        fun of(at: ZonedDateTime, now: ZonedDateTime): Horizon =
            when (ChronoUnit.DAYS.between(now.toLocalDate(), at.toLocalDate())) {
                0L -> Today
                1L -> Tomorrow
                in 2L..6L -> ThisWeek
                else -> Later
            }
    }
}

/** "today at 6:30 PM", "tomorrow at 7:30 AM", "Sat at 9 AM" within the week, else "30 Oct at 9 AM". */
fun whenWords(at: ZonedDateTime, now: ZonedDateTime): String {
    val day =
        when (Horizon.of(at, now)) {
            Horizon.Today -> "today"
            Horizon.Tomorrow -> "tomorrow"
            Horizon.ThisWeek -> at.format(WEEKDAY)
            Horizon.Later -> at.format(DATE)
        }
    return "$day at ${clockTime(at.toLocalTime())}"
}

/** A routine's line under its name: whether it is paused, when it runs next, and whether its last run failed. */
fun routineStatus(routine: Routine, now: ZonedDateTime): String {
    val next = nextRun(routine, now)
    val status =
        when {
            !routine.on -> "Paused"
            next != null -> "Next: ${whenWords(next, now)}"
            routine.next != null -> "Due now"
            else -> "On"
        }
    return if (routine.lastRun?.outcome == RunOutcome.Failed) "$status · Last run failed" else status
}

/** The routines page's order: each that is on by its next run, soonest (or overdue) first, then the rest as the roster has them. */
fun soonestFirst(routines: List<Routine>): List<Routine> = routines.sortedWith(compareBy(nullsLast()) { routine -> routine.next.takeIf { routine.on } })

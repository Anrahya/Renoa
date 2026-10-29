package live.renoa.app.home

import live.renoa.app.agents.Routine
import live.renoa.app.agents.RunOutcome
import live.renoa.app.agents.clockTime
import java.time.LocalTime
import java.time.ZonedDateTime
import java.time.format.DateTimeFormatter
import java.time.temporal.ChronoUnit
import java.util.Locale

/** The day strip runs from 6 am to midnight: the waking day, not the clock's. */
private const val STRIP_STARTS = 6
private const val STRIP_HOURS = 18

private val WEEKDAY_CLOCK = DateTimeFormatter.ofPattern("EEE h:mm a", Locale.ENGLISH)
private val WEEKDAY = DateTimeFormatter.ofPattern("EEE", Locale.ENGLISH)
private val DATE = DateTimeFormatter.ofPattern("d MMM", Locale.ENGLISH)

fun greeting(hour: Int): String =
    when (hour) {
        in 5..11 -> "Good morning."
        in 12..16 -> "Good afternoon."
        in 17..21 -> "Good evening."
        else -> "Good night."
    }

/** "Thu 7:40 PM". */
fun clockWords(now: ZonedDateTime): String = now.format(WEEKDAY_CLOCK)

/** Where [time] sits on the day strip: 0 at 6 am, 1 at midnight. Earlier hours sit at the start. */
fun stripAt(time: LocalTime): Float = ((time.toSecondOfDay() / 3600f - STRIP_STARTS) / STRIP_HOURS).coerceIn(0f, 1f)

/**
 * When [routine] runs next, if that is still to come. Renoa updates a routine's next run on its own
 * clock, so for a moment after a run the next one it last said can already be past.
 */
fun nextRun(routine: Routine, now: ZonedDateTime): ZonedDateTime? = routine.next?.atZone(now.zone)?.takeIf { !it.isBefore(now) }

/** A routine on today's strip: a run that ended today ([ran] is how), or its next run, still to come today ([ran] is null). */
data class DayMark(val routine: Routine, val at: ZonedDateTime, val ran: RunOutcome?)

/** Today's runs and today's next runs, in time order. */
fun dayMarks(routines: List<Routine>, now: ZonedDateTime): List<DayMark> {
    val today = now.toLocalDate()
    return routines
        .flatMap { routine ->
            listOfNotNull(
                routine.lastRun?.let { run -> run.at.atZone(now.zone).takeIf { it.toLocalDate() == today }?.let { DayMark(routine, it, run.outcome) } },
                nextRun(routine, now)?.takeIf { it.toLocalDate() == today }?.let { DayMark(routine, it, null) },
            )
        }.sortedBy { it.at }
}

/** "Next: GitHub check at 6:30 PM", for the first run still to come today. */
fun nextWords(routines: List<Routine>, now: ZonedDateTime): String =
    dayMarks(routines, now).firstOrNull { it.ran == null }?.let { "Next: ${it.routine.name} at ${clockTime(it.at.toLocalTime())}" }
        ?: "Nothing else is set for today"

/** How far off a day is: the Upcoming page's headings, and how a time names its day. */
enum class Horizon(val heading: String) {
    Today("Later today"),
    Tomorrow("Tomorrow"),
    ThisWeek("This week"),
    Later("Later"),
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

/** One heading of the Upcoming page and the runs under it, each with its time as the row shows it. */
data class UpcomingGroup(val horizon: Horizon, val runs: List<Pair<Routine, String>>)

/** Every routine's next run from now on, under its [Horizon], soonest first. */
fun upcoming(routines: List<Routine>, now: ZonedDateTime): List<UpcomingGroup> =
    routines
        .mapNotNull { r -> nextRun(r, now)?.let { r to it } }
        .sortedBy { it.second }
        .groupBy { (_, at) -> Horizon.of(at, now) }
        .map { (horizon, group) ->
            UpcomingGroup(
                horizon,
                group.map { (routine, at) ->
                    routine to
                        when (horizon) {
                            Horizon.Today, Horizon.Tomorrow -> clockTime(at.toLocalTime())
                            Horizon.ThisWeek -> "${at.format(WEEKDAY)} ${clockTime(at.toLocalTime())}"
                            Horizon.Later -> at.format(DATE)
                        }
                },
            )
        }

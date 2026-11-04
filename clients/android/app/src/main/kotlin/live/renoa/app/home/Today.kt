package live.renoa.app.home

import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.Feed
import live.renoa.app.agents.NoteKind
import live.renoa.app.agents.Routine
import live.renoa.app.agents.RunOutcome
import live.renoa.app.agents.clockTime
import java.time.Instant
import java.time.LocalTime
import java.time.ZonedDateTime
import java.time.format.DateTimeFormatter
import java.time.temporal.ChronoUnit
import java.util.Locale
import kotlin.math.abs

/** The day strip runs from 6 am to midnight: the waking day, not the clock's. */
private const val STRIP_STARTS = 6
private const val STRIP_ENDS = 24

/** How much of the strip's width the two hours round now take, so a busy evening does not pile up. */
private const val LENS = 0.36f

private val WEEKDAY = DateTimeFormatter.ofPattern("EEE", Locale.ENGLISH)
private val DATE = DateTimeFormatter.ofPattern("d MMM", Locale.ENGLISH)

/**
 * The day strip's scale on [now]: 6 am at 0 and midnight at 1, with the hour either side of now
 * spread over [LENS] of the width and the rest of the day sharing what is left. Earlier hours sit
 * at the start.
 */
class DayLens(now: LocalTime) {
    private val hour = hours(now).coerceIn(STRIP_STARTS.toFloat(), STRIP_ENDS.toFloat())

    /** Where the magnified hours start and end. */
    val from: Float = (hour - 1).coerceAtLeast(STRIP_STARTS.toFloat())
    val to: Float = (hour + 1).coerceAtMost(STRIP_ENDS.toFloat())

    private val restHours = (from - STRIP_STARTS) + (STRIP_ENDS - to)

    /** Where [time] sits, from 0 to 1. */
    fun at(time: LocalTime): Float = atHour(hours(time).coerceIn(STRIP_STARTS.toFloat(), STRIP_ENDS.toFloat()))

    fun atHour(h: Float): Float {
        val rest = 1 - LENS
        return when {
            h <= from -> (h - STRIP_STARTS) / restHours * rest
            h <= to -> (from - STRIP_STARTS) / restHours * rest + (h - from) / (to - from) * LENS
            else -> (from - STRIP_STARTS) / restHours * rest + LENS + (h - to) / restHours * rest
        }
    }

    /** The hour of the day at [x], from 0 to 1: [at] turned round. */
    fun hourAt(x: Float): Float {
        val rest = 1 - LENS
        val start = atHour(from)
        val end = atHour(to)
        return when {
            x <= start -> STRIP_STARTS + x / rest * restHours
            x <= end -> from + (x - start) / LENS * (to - from)
            else -> to + (x - end) / rest * restHours
        }
    }

    private companion object {
        fun hours(time: LocalTime): Float = time.toSecondOfDay() / 3600f
    }
}

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

/**
 * What a mark on the day strip is: something an agent left the owner (above the bar), or a
 * routine's run (below it). Of marks drawn as one, the first kind here leads.
 */
enum class MarkKind(val left: Boolean) {
    Ask(true),
    Problem(true),
    Note(true),
    Ran(false),
    Failed(false),
    Skipped(false),
    Coming(false),
}

/** One mark on the day strip, from the agent [agentId] (null while it is not on the roster), and what it says. */
data class StripMark(val key: String, val at: ZonedDateTime, val agentId: String?, val kind: MarkKind, val words: String)

/**
 * Today's marks in time order: each open question and each note in the feed, then each routine's
 * run today and its next run still to come today.
 */
fun stripMarks(roster: List<AgentSummary>, feed: Feed, routines: List<Routine>, now: ZonedDateTime): List<StripMark> {
    val today = now.toLocalDate()
    fun local(at: Instant) = at.atZone(now.zone).takeIf { it.toLocalDate() == today }
    fun name(id: String?) = roster.firstOrNull { it.id == id }?.name
    val asks =
        roster.mapNotNull { agent ->
            agent.glance.ask?.let { ask -> ask.at?.let(::local)?.let { StripMark("${agent.id}/ask", it, agent.id, MarkKind.Ask, "${agent.name} asks: ${ask.question}") } }
        }
    val notes =
        feed.notes.mapNotNull { note ->
            local(note.at)?.let {
                val problem = note.kind == NoteKind.Problem
                val words = if (problem) note.words else listOfNotNull(name(note.agentId), note.words).joinToString(": ")
                StripMark(note.id, it, note.agentId, if (problem) MarkKind.Problem else MarkKind.Note, words)
            }
        }
    val runs =
        dayMarks(routines, now).map { mark ->
            val at = clockTime(mark.at.toLocalTime())
            val name = mark.routine.name
            val (kind, words) =
                when (mark.ran) {
                    null -> MarkKind.Coming to "$name at $at"
                    RunOutcome.Done -> MarkKind.Ran to "$name ran at $at"
                    RunOutcome.Failed -> MarkKind.Failed to "$name failed at $at"
                    RunOutcome.Skipped -> MarkKind.Skipped to "$name was skipped at $at"
                }
            StripMark("${mark.routine.id}/${if (mark.ran == null) "next" else "ran"}", mark.at, mark.routine.agentId, kind, words)
        }
    return (asks + notes + runs).sortedBy { it.at }
}

/**
 * Groups [items], in the order given, into runs whose neighbours sit closer than [within] by [x]:
 * marks drawn too close to tell apart show as one, with a count.
 */
fun <T> clustered(items: List<T>, x: (T) -> Float, within: Float): List<List<T>> {
    val groups = mutableListOf<MutableList<T>>()
    for (item in items.sortedBy(x)) {
        val last = groups.lastOrNull()
        if (last != null && x(item) - x(last.last()) < within) last += item else groups += mutableListOf(item)
    }
    return groups
}

/** The mark nearest [x] on [lens], if one sits within [within] of it. */
fun nearest(marks: List<StripMark>, lens: DayLens, x: Float, within: Float): StripMark? =
    marks.minByOrNull { abs(lens.at(it.at.toLocalTime()) - x) }?.takeIf { abs(lens.at(it.at.toLocalTime()) - x) <= within }

/** The line under the strip: what runs next, and how today's runs went. */
fun stripLine(marks: List<StripMark>, routines: List<Routine>, now: ZonedDateTime): String {
    val ran = marks.count { it.kind == MarkKind.Ran }
    val failed = marks.count { it.kind == MarkKind.Failed }
    return listOfNotNull(nextWords(routines, now), "$ran ran".takeIf { ran > 0 }, "$failed didn’t finish".takeIf { failed > 0 }).joinToString(" · ")
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

package live.renoa.app.agents

import java.time.DateTimeException
import java.time.LocalTime
import java.time.OffsetDateTime
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.DateTimeParseException
import java.util.Locale

/** What a schedule reads when its shape is not one [scheduleWords] knows. */
internal const val ON_A_SCHEDULE = "On a schedule"

private val DAYS = listOf("Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday")

/** The Host's three-letter weekday names, which it reads in any case, Sunday first as 0. */
private val DAY_NAMES = listOf("SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT")
private val WEEK_ORDER = listOf(1, 2, 3, 4, 5, 6, 0)
private val CLOCK = DateTimeFormatter.ofPattern("h:mm a", Locale.ENGLISH)
private val HOUR = DateTimeFormatter.ofPattern("h a", Locale.ENGLISH)
private val DATE = DateTimeFormatter.ofPattern("EEE d MMM", Locale.ENGLISH)

/**
 * A Host schedule in plain words, such as "Every day at 6:30 PM", "Every weekday at 7:30 AM" or
 * "Every Thursday at 7:15 PM". A cron schedule's times are read in its own timezone, which is named
 * when its clock differs from [here]'s; a one-off time is shown in [here]. A shape this does not
 * know reads [ON_A_SCHEDULE] rather than a guess.
 */
internal fun scheduleWords(schedule: ObservedSchedule, here: ZoneId): String =
    when (schedule.kind) {
        "once" -> schedule.at?.let { once(it, here) }
        "cron" -> schedule.expression?.let(::cron)?.let { it + zoneNote(schedule.timezone, here) }
        else -> null
    } ?: ON_A_SCHEDULE

private fun once(at: String, here: ZoneId): String? =
    try {
        val time = OffsetDateTime.parse(at).atZoneSameInstant(here)
        "Once, on ${time.format(DATE)} at ${clock(time.toLocalTime())}"
    } catch (unreadable: DateTimeParseException) {
        null
    }

/** The five fields "minute hour day-of-month month weekday", for the shapes people set most. */
private fun cron(expression: String): String? {
    val fields = expression.trim().split(Regex("\\s+"))
    if (fields.size != 5) return null
    val (minute, hour, day, month, weekday) = fields
    val m = minute.toIntOrNull()?.takeIf { it in 0..59 } ?: return null
    if (month != "*") return null
    val h = hour.toIntOrNull()?.takeIf { it in 0..23 }
    if (h == null) {
        if (day != "*" || weekday != "*") return null
        return when {
            hour == "*" -> if (m == 0) "Every hour, on the hour" else "Every hour at $m minutes past"
            hour.startsWith("*/") -> hour.removePrefix("*/").toIntOrNull()?.takeIf { it in 2..23 }?.let { "Every $it hours" }
            else -> null
        }
    }
    val at = "at ${clock(LocalTime.of(h, m))}"
    return when {
        day == "*" && weekday == "*" -> "Every day $at"
        day == "*" -> days(weekday)?.let { "$it $at" }
        weekday == "*" -> day.toIntOrNull()?.takeIf { it in 1..31 }?.let { "On the ${ordinal(it)} of every month $at" }
        else -> null
    }
}

/** A weekday field as words, "Every weekday" or "Every Monday and Friday", or null for steps and other shapes. */
private fun days(field: String): String? {
    val chosen = mutableSetOf<Int>()
    for (part in field.split(',')) {
        val bounds = part.split('-').map { weekday(it) ?: return null }
        when (bounds.size) {
            1 -> chosen += bounds[0] % 7
            2 -> if (bounds[0] <= bounds[1]) (bounds[0]..bounds[1]).forEach { chosen += it % 7 } else return null
            else -> return null
        }
    }
    return when (chosen) {
        (0..6).toSet() -> "Every day"
        (1..5).toSet() -> "Every weekday"
        else -> "Every " + listed(WEEK_ORDER.filter { it in chosen }.map { DAYS[it] })
    }
}

/** A weekday as the Host reads it: 0 to 7, where 7 is Sunday like 0, or a three-letter name. */
private fun weekday(text: String): Int? =
    text.toIntOrNull()?.takeIf { it in 0..7 } ?: DAY_NAMES.indexOfFirst { it.equals(text, ignoreCase = true) }.takeIf { it >= 0 }

private fun listed(words: List<String>): String = if (words.size < 3) words.joinToString(" and ") else words.dropLast(1).joinToString(", ") + " and " + words.last()

private fun clock(time: LocalTime): String = time.format(if (time.minute == 0) HOUR else CLOCK)

private fun ordinal(n: Int): String =
    n.toString() +
        when {
            n % 100 in 11..13 -> "th"
            n % 10 == 1 -> "st"
            n % 10 == 2 -> "nd"
            n % 10 == 3 -> "rd"
            else -> "th"
        }

/** ", Kolkata time" when the schedule's timezone keeps a different clock from [here]; nothing when it is the same. */
private fun zoneNote(timezone: String?, here: ZoneId): String {
    val zone =
        try {
            ZoneId.of(timezone ?: return "")
        } catch (unknown: DateTimeException) {
            return ""
        }
    if (zone.rules == here.rules) return ""
    return ", ${zone.id.substringAfterLast('/').replace('_', ' ')} time"
}

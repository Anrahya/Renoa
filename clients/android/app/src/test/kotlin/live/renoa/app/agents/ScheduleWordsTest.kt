package live.renoa.app.agents

import java.time.ZoneId
import kotlin.test.Test
import kotlin.test.assertEquals

class ScheduleWordsTest {
    private val kolkata = ZoneId.of("Asia/Kolkata")

    private fun cron(expression: String, timezone: String = "Asia/Kolkata") = scheduleWords(ObservedSchedule("cron", expression = expression, timezone = timezone), kolkata)

    @Test
    fun theSchedulesPeopleSetReadAsPlainWords() {
        val expected =
            mapOf(
                "30 18 * * *" to "Every day at 6:30 PM",
                "0 13 * * *" to "Every day at 1 PM",
                "0 0 * * *" to "Every day at 12 AM",
                "30 7 * * 1-5" to "Every weekday at 7:30 AM",
                "30 7 * * mon-FRI" to "Every weekday at 7:30 AM",
                "15 19 * * 4" to "Every Thursday at 7:15 PM",
                "0 10 * * 0" to "Every Sunday at 10 AM",
                "0 10 * * 7" to "Every Sunday at 10 AM",
                "0 10 * * 0,6" to "Every Saturday and Sunday at 10 AM",
                "0 8 * * 1,3,5" to "Every Monday, Wednesday and Friday at 8 AM",
                "0 8 * * 0-6" to "Every day at 8 AM",
                "0 9 1 * *" to "On the 1st of every month at 9 AM",
                "0 9 22 * *" to "On the 22nd of every month at 9 AM",
                "0 9 13 * *" to "On the 13th of every month at 9 AM",
                "0 9 28 * *" to "On the 28th of every month at 9 AM",
                "0 * * * *" to "Every hour, on the hour",
                "20 * * * *" to "Every hour at 20 minutes past",
                "0 */3 * * *" to "Every 3 hours, on the hour",
                "45 */2 * * *" to "Every 2 hours at 45 minutes past",
                "0 */12 * * *" to "Every 12 hours, on the hour",
            )
        for ((expression, words) in expected) assertEquals(words, cron(expression), expression)
    }

    @Test
    fun aShapeItDoesNotKnowIsNotGuessed() {
        val unknown =
            listOf(
                "*/10 * * * *", "0 9 * 1 *", "0 9 1 * 1", "0 9 * * 1/2", "0 9 * * 5-1", "0 9 * * 8", "0 9-17 * * *", "0 9 * *", "0 24 * * *",
                // A step from midnight that does not divide the day leaves a shorter gap before midnight.
                "0 */1 * * *", "0 */5 * * *", "0 */7 * * *", "0 */16 * * *",
                // Months without that day skip it.
                "0 9 29 * *", "0 9 31 * *",
            )
        for (expression in unknown) assertEquals(ON_A_SCHEDULE, cron(expression), expression)
        assertEquals(ON_A_SCHEDULE, scheduleWords(ObservedSchedule("weekly"), kolkata))
        assertEquals(ON_A_SCHEDULE, scheduleWords(ObservedSchedule("once", at = "tomorrow"), kolkata))
    }

    @Test
    fun aTimezoneWithAnotherClockIsNamed() {
        assertEquals("Every day at 9 AM, New York time", cron("0 9 * * *", "America/New_York"))
        assertEquals("Every day at 9 AM", cron("0 9 * * *", "Asia/Calcutta"), "another name for the same clock is not named")
        assertEquals(ON_A_SCHEDULE, cron("0 9 * * *", "Not/AZone"), "a timezone the phone cannot read is not shown on the phone's clock")
        assertEquals(ON_A_SCHEDULE, scheduleWords(ObservedSchedule("cron", expression = "0 9 * * *"), kolkata))
    }

    @Test
    fun aOneOffTimeIsShownOnThePhonesClock() {
        assertEquals("Once, on Fri 30 Oct at 7 PM", scheduleWords(ObservedSchedule("once", at = "2026-10-30T13:30:00Z"), kolkata))
    }
}

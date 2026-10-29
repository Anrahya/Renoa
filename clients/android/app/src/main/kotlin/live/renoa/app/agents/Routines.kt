package live.renoa.app.agents

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import live.renoa.rcp.HostMethod
import java.time.Instant
import java.time.ZoneId

/** How a routine's latest finished run ended. The names are the Host's run statuses. */
@Serializable
enum class RunOutcome {
    /** The agent finished it, even if some of its tool calls failed. */
    @SerialName("succeeded")
    Done,

    @SerialName("failed")
    Failed,

    /** It came due too late to be worth running, so it never ran. */
    @SerialName("skipped")
    Skipped,
}

@Serializable
data class LastRun(@Serializable(with = InstantMillis::class) val at: Instant, val outcome: RunOutcome)

/**
 * Something an agent does on a schedule, as the owner sees it: an automation on the Host.
 * [agentId] is the roster's id for the agent that runs it, or null while that agent is not on the roster.
 */
@Serializable
data class Routine(
    val id: String,
    val agentId: String?,
    val name: String,
    /** When it runs, in plain words, such as "Every day at 6:30 PM". */
    val schedule: String,
    val on: Boolean,
    /** When it runs next, or null while it is off. */
    @Serializable(with = InstantMillis::class) val next: Instant?,
    val lastRun: LastRun?,
)

/** The fields of an automation in the Host's observation (`GET /v1/host`) that the app reads. */
@Serializable
internal data class ObservedAutomation(
    val id: String,
    @SerialName("agent_id") val agentId: String,
    val name: String,
    val schedule: ObservedSchedule,
    val enabled: Boolean,
    val revision: Long,
    @SerialName("next_due_ms") val nextDueMs: Long,
    @SerialName("last_run") val lastRun: ObservedRun? = null,
) {
    fun routine(rosterId: String?, here: ZoneId): Routine =
        Routine(
            id,
            rosterId,
            name,
            scheduleWords(schedule, here),
            enabled,
            next = Instant.ofEpochMilli(nextDueMs).takeIf { enabled },
            lastRun = lastRun?.let { LastRun(Instant.ofEpochMilli(it.finishedAtMs ?: it.dueMs), it.status) },
        )
}

/** A Host schedule: `once` carries [at]; `cron` carries [expression], read in [timezone]. */
@Serializable
internal data class ObservedSchedule(val kind: String, val at: String? = null, val expression: String? = null, val timezone: String? = null)

@Serializable
internal data class ObservedRun(
    val status: RunOutcome,
    @SerialName("due_ms") val dueMs: Long,
    /** Null for runs recorded before the Host kept it. */
    @SerialName("finished_at_ms") val finishedAtMs: Long? = null,
)

@Serializable
private data class Observation(val automations: List<ObservedAutomation>)

@Serializable
private data class Enablement(val revision: Long, val enabled: Boolean, @SerialName("next_due_ms") val nextDueMs: Long)

/** The Host's automations: read from its observation, and paused or resumed by the owner. */
internal class HostRoutines(private val host: HostClient) {
    suspend fun observe(): List<ObservedAutomation> = host.call(HostMethod.Get, "/v1/host", null, Observation.serializer()).automations

    /**
     * Turns [automation] on or off from the revision the app last read, so a change made elsewhere
     * meanwhile is a conflict, not an overwrite. Returns [automation] with the revision, state and
     * next run from the Host's receipt; a repeated [operationId] gets the first receipt back, which
     * can be older than a later change.
     */
    suspend fun setEnabled(automation: ObservedAutomation, on: Boolean, operationId: String): ObservedAutomation {
        val body =
            buildJsonObject {
                put("operation_id", operationId)
                put("expected_revision", automation.revision)
                put("enabled", on)
            }
        val changed = host.call(HostMethod.Post, "/v1/host/automations/${automation.id}/enabled", body.toString(), Enablement.serializer())
        return automation.copy(revision = changed.revision, enabled = changed.enabled, nextDueMs = changed.nextDueMs)
    }
}

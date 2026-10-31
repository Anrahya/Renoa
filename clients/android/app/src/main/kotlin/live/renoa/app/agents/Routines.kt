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

/**
 * The parts of the Host's observation (`GET /v1/host`) the app reads on each roster check. The main
 * agent is required, so a Host that does not say is not read as naming none.
 */
@Serializable
internal data class Observation(
    val automations: List<ObservedAutomation>,
    @SerialName("main_agent") val mainAgent: ObservedMainAgent?,
)

/** The Host's main agent, by its Host agent id. */
@Serializable
internal data class ObservedMainAgent(@SerialName("main_agent_id") val id: String)

@Serializable
private data class Enablement(val revision: Long, val enabled: Boolean, @SerialName("next_due_ms") val nextDueMs: Long)

/** The Host's automations: read from its observation, and paused or resumed by the owner. */
internal class HostRoutines(private val host: HostClient) {
    suspend fun observe(): Observation = host.call(HostMethod.Get, "/v1/host", null, Observation.serializer())

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

/**
 * The automations as the Host last listed them, by id, with the revision a change starts from. The
 * Host answers calls in any order, so a routine held at a newer revision keeps it. [changed] runs
 * after each change to what is held.
 */
internal class HeldRoutines(private val host: HostRoutines, private val changed: () -> Unit) {
    private val lock = Any()
    private val automations = LinkedHashMap<String, ObservedAutomation>()

    /** Set once the Host has listed the automations; until then the kept routines show. */
    private var heard = false

    /**
     * Reads the routines again and returns the observation they came in, for the caller's other
     * reads. While the Host cannot be read it returns null, and the last routines stay.
     */
    suspend fun read(): Observation? {
        val observed =
            try {
                host.observe()
            } catch (failed: RepositoryException) {
                return null
            }
        val listed = observed.automations
        synchronized(lock) {
            val held = automations.toMap()
            automations.clear()
            listed.forEach { read -> automations[read.id] = held[read.id]?.takeIf { it.revision > read.revision } ?: read }
            heard = true
        }
        changed()
        return observed
    }

    /** Turns the routine on or off from the revision held. A refusal that says it changed elsewhere reads it again first. */
    suspend fun set(routineId: String, on: Boolean, operationId: String) {
        val automation = synchronized(lock) { automations[routineId] ?: throw RepositoryException(notHeld(heard), "no routine $routineId") }
        if (automation.enabled == on) return
        val receipt =
            try {
                host.setEnabled(automation, on, operationId)
            } catch (refused: RepositoryException) {
                if (refused.problem == Problem.Conflict || refused.problem == Problem.NotFound) read()
                throw refused
            }
        synchronized(lock) {
            // A read that answered meanwhile may have dropped the routine or seen a newer revision; either stands.
            val held = automations[routineId]
            if (held != null && held.revision < receipt.revision) automations[routineId] = receipt
        }
        changed()
    }

    /** The routines as the owner sees them, each named by its agent's roster id; null until the Host has listed them. */
    fun shown(rosterId: (agentId: String) -> String?, here: ZoneId): List<Routine>? =
        synchronized(lock) { automations.values.map { it.routine(rosterId(it.agentId), here) }.takeIf { heard } }
}

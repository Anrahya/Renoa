package live.renoa.app.agents

import kotlinx.coroutines.CancellationException
import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import live.renoa.rcp.HostAnswer
import live.renoa.rcp.HostMethod
import live.renoa.rcp.RcpException
import live.renoa.rcp.RcpSurface
import kotlin.uuid.Uuid

/** Every agent the app makes starts from this Host preset. */
internal const val APP_PRESET = "renoa.general.v1"

/**
 * The fields of the Host's agent definition the app reads. The definition's shape is owned by the
 * Host and published as `docs/schemas/agent-definition.schema.json`; a test checks these against it.
 */
@Serializable
internal data class AgentRecord(val id: String, val name: String, val operational: Operational) {
    @Serializable
    data class Operational(val purpose: String? = null)
}

@Serializable
private data class Created(@SerialName("operation_id") val operationId: String, val record: AgentRecord)

private val hostJson = Json { ignoreUnknownKeys = true }

/**
 * The Host's agent operations, called over the app's RCP connection. Every failure is a
 * [RepositoryException]: a refusal names what was wrong, and anything else is [Problem.Unavailable].
 */
internal class HostAgents(private val surface: RcpSurface) {
    /** Makes an agent, or returns the one an earlier call under [operationId] made. */
    suspend fun create(operationId: String, name: String, purpose: String?): AgentRecord {
        val body =
            buildJsonObject {
                put("operation_id", operationId)
                put("preset_id", APP_PRESET)
                put("name", name)
                purpose?.let { put("purpose", it) }
            }
        return decode(call(HostMethod.Post, "/v1/host/agents", body.toString()), Created.serializer()).record
    }

    suspend fun definition(agentId: Uuid): AgentRecord = decode(call(HostMethod.Get, "/v1/host/agents/$agentId", null), AgentRecord.serializer())

    private suspend fun call(method: HostMethod, path: String, body: String?): HostAnswer =
        try {
            surface.callHost(method, path, body)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (refused: RcpException) {
            throw refused.asProblem()
        } catch (lost: Exception) {
            // Not connected, the connection lost, or a protocol failure that ended it.
            throw RepositoryException(Problem.Unavailable, lost.message.orEmpty())
        }

    private fun <T> decode(answer: HostAnswer, shape: KSerializer<T>): T {
        val problem =
            when (answer.status) {
                in 200..299 -> null
                404 -> Problem.NotFound
                409 -> Problem.Conflict
                400, 422 -> Problem.Invalid
                else -> Problem.Unavailable
            }
        if (problem != null) throw RepositoryException(problem, "the Host answered ${answer.status}: ${answer.body}")
        return try {
            hostJson.decodeFromString(shape, answer.body)
        } catch (malformed: IllegalArgumentException) {
            // Every decoding failure, a missing field among them, is one of these.
            throw RepositoryException(Problem.Unavailable, "the Host's answer is not an agent: ${malformed.message}")
        }
    }
}

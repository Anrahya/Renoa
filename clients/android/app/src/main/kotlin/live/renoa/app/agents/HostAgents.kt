package live.renoa.app.agents

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import live.renoa.rcp.HostMethod
import kotlin.uuid.Uuid

/** Every agent the app makes starts from this Host preset. */
internal const val APP_PRESET = "renoa.app.v3"

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

/** The Host's agent operations. */
internal class HostAgents(private val host: HostClient) {
    /** Makes an agent, or returns the one an earlier call under [operationId] made. */
    suspend fun create(operationId: String, name: String, purpose: String?): AgentRecord {
        val body =
            buildJsonObject {
                put("operation_id", operationId)
                put("preset_id", APP_PRESET)
                put("name", name)
                purpose?.let { put("purpose", it) }
            }
        return host.call(HostMethod.Post, "/v1/host/agents", body.toString(), Created.serializer()).record
    }

    suspend fun definition(agentId: Uuid): AgentRecord = host.call(HostMethod.Get, "/v1/host/agents/$agentId", null, AgentRecord.serializer())
}

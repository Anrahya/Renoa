package live.renoa.app.agents

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import live.renoa.rcp.HostMethod
import kotlin.uuid.Uuid

/** Every agent the app makes starts from this Host preset. */
internal const val APP_PRESET = "renoa.app.v5"

/**
 * The fields of the Host's agent definition the app reads. The definition's shape is owned by the
 * Host and published as `docs/schemas/agent-definition.schema.json`; a test checks these against it.
 */
@Serializable
internal data class AgentRecord(val id: String, val name: String, val operational: Operational) {
    @Serializable
    data class Operational(val purpose: String? = null)
}

/**
 * An app an agent can use: a connection on the Host, named by the plugin that serves it, if any.
 * Switching it off keeps its sign-in, so switching it on again needs none.
 */
@Serializable
data class AgentApp(val connection: String, val name: String? = null, val endpoint: String, val on: Boolean)

@Serializable
private data class Apps(val apps: List<AgentApp>)

/** How an agent looks, and the revision of a picture the owner set over it, if any. */
@Serializable
internal data class Appearance(val look: Look, @SerialName("picture_revision") val pictureRevision: String? = null)

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

    suspend fun appearance(agentId: Uuid): Appearance = host.call(HostMethod.Get, "/v1/host/agents/$agentId/appearance", null, Appearance.serializer())

    suspend fun apps(agentId: Uuid): List<AgentApp> = host.call(HostMethod.Get, "/v1/host/agents/$agentId/apps", null, Apps.serializer()).apps

    /** Gives the agent [connection] or takes it away; asking twice is asking once. Returns the agent's apps after. */
    suspend fun setApp(agentId: Uuid, connection: String, on: Boolean): List<AgentApp> {
        val body = buildJsonObject { put("enabled", on) }
        return host.call(HostMethod.Post, "/v1/host/agents/$agentId/apps/${connection.encodedSegment()}/enabled", body.toString(), Apps.serializer()).apps
    }
}

/** [this] as one path segment, so a connection id cannot reach another path. */
private fun String.encodedSegment(): String = java.net.URLEncoder.encode(this, "UTF-8").replace("+", "%20")

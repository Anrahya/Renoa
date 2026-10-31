package live.renoa.app.agents

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import live.renoa.rcp.HostMethod
import kotlin.uuid.Uuid

/** The owner's profile: the `USER.md` every agent that talks to them reads, and the revision a change must name. */
@Serializable
data class OwnerProfile(val content: String, val revision: String)

@Serializable
private data class Made(val agents: List<JsonObject>)

/** The Host's main agent in its observation. Required, so a Host that does not say is not read as naming none. */
@Serializable
private data class Named(@SerialName("main_agent") val mainAgent: MainAgent?) {
    @Serializable
    data class MainAgent(@SerialName("main_agent_id") val id: String)
}

/** The owner's side of the Host: their profile, whether any agent has been made for them, and which one is main. */
internal class HostOwner(private val host: HostClient) {
    suspend fun profile(): OwnerProfile = host.call(HostMethod.Get, "/v1/host/profile", null, OwnerProfile.serializer())

    /** Replaces the profile. The Host answers a repeat of a save that already landed with the same profile. */
    suspend fun saveProfile(expectedRevision: String, content: String): OwnerProfile {
        val body =
            buildJsonObject {
                put("expected_revision", expectedRevision)
                put("content", content)
            }
        return host.call(HostMethod.Put, "/v1/host/profile", body.toString(), OwnerProfile.serializer())
    }

    /** Read from the Host's own list, because a node that is offline advertises no agents at all. */
    suspend fun hasAgents(): Boolean = host.call(HostMethod.Get, "/v1/host", null, Made.serializer()).agents.isNotEmpty()

    /** The Host's id for its main agent, or null while the Host has no agent. */
    suspend fun mainAgent(): Uuid? {
        val named = host.call(HostMethod.Get, "/v1/host", null, Named.serializer()).mainAgent ?: return null
        return Uuid.parseOrNull(named.id) ?: throw RepositoryException(Problem.Unavailable, "the Host's main agent ${named.id} is not an agent id")
    }
}

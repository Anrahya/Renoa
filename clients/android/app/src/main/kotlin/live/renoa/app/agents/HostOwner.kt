package live.renoa.app.agents

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import live.renoa.rcp.HostMethod

/** The owner's profile: the `USER.md` every agent that talks to them reads, and the revision a change must name. */
@Serializable
data class OwnerProfile(val content: String, val revision: String)

/**
 * A note an agent keeps about the owner, in its own words. [written] is the day, `YYYY-MM-DD`;
 * [happened] the day it says something happened, when that differs; [due] when it asks to be
 * brought up, `YYYY-MM-DDTHH:MM` in the owner's zone.
 */
@Serializable
data class MemoryNote(val id: Long, val written: String, val author: String? = null, val text: String, val happened: String? = null, val due: String? = null)

/** The newest notes agents keep about the owner, newest first, and how many there are in all. */
@Serializable
data class OwnerMemory(val notes: List<MemoryNote>, val total: Long)

@Serializable
private data class Made(val agents: List<JsonObject>)

/** The owner's side of the Host: their profile, and whether any agent has been made for them. */
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

    suspend fun memory(): OwnerMemory = host.call(HostMethod.Get, "/v1/host/memory", null, OwnerMemory.serializer())

    /** Read from the Host's own list, because a node that is offline advertises no agents at all. */
    suspend fun hasAgents(): Boolean = host.call(HostMethod.Get, "/v1/host", null, Made.serializer()).agents.isNotEmpty()
}

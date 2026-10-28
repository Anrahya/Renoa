package live.renoa.app.agents

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.StateFlow

/** An agent with everything the edit screen shows. */
data class AgentProfile(
    val id: String,
    val name: String,
    val purpose: String,
    val abilities: List<AbilitySwitch>,
)

/**
 * A request to make an agent. [operationId] is chosen when the screen opens, so sending the
 * same request twice makes one agent.
 */
data class CreateAgent(
    val operationId: String,
    val name: String,
    val purpose: String,
    val litAbilities: Set<String>,
)

/** A change to name and purpose. The expected values make a stale edit a conflict, not a silent overwrite. */
data class EditProfile(
    val expectedName: String,
    val name: String,
    val expectedPurpose: String,
    val purpose: String,
)

enum class Problem {
    NotFound,
    Conflict,
    Invalid,

    /** Renoa could not be reached, so whether the request took effect is unknown. */
    Unavailable,
    ;

    /** What the owner reads. The refusal's own message is for logs: it may name fields and values. */
    val ownerWords: String
        get() =
            when (this) {
                NotFound -> "That is gone."
                Conflict -> "This was changed somewhere else. Close it and open it again."
                Invalid -> "That could not be saved. Check what you entered."
                Unavailable -> "Could not reach Renoa. Check your connection and try again."
            }
}

/** A refusal, or a failure to reach the other side. */
class RepositoryException(val problem: Problem, message: String) : Exception(message)

/**
 * Everything the screens need from Renoa. The app runs on a fake until the backend tracks
 * land (agent profiles, streaming chat, stop); a real implementation replaces it behind this
 * interface without touching a screen.
 */
interface AgentRepository {
    val agents: StateFlow<List<AgentSummary>>

    fun conversation(agentId: String): Flow<Conversation>

    /** Returns once the message is kept. The reply arrives later through [conversation]. */
    suspend fun send(agentId: String, text: String)

    /** Stops what the agent is doing now. It stays stopped. */
    suspend fun stop(agentId: String)

    suspend fun profile(agentId: String): AgentProfile

    /** The abilities a new agent starts with, so the create screen can show them before the agent exists. */
    suspend fun starterAbilities(): List<AbilitySwitch>

    suspend fun create(request: CreateAgent): AgentProfile

    suspend fun edit(agentId: String, edit: EditProfile): AgentProfile

    suspend fun setAbility(agentId: String, abilityId: String, lit: Boolean): AbilitySwitch
}

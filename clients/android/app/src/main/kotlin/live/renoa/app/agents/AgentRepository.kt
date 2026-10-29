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

/** How the app is connected to Renoa. */
enum class Link {
    /** The first connection is being made. */
    Connecting,
    Connected,

    /** The connection failed or dropped; it is being made again. */
    Lost,

    /** Renoa refused this phone's sign-in, as after it was signed out elsewhere. Only signing in again helps. */
    SignedOut,
}

/** A refusal, or a failure to reach the other side. */
class RepositoryException(val problem: Problem, message: String) : Exception(message)

/** Not reaching Renoa has its own words; any other refusal gets the one that fits the screen asking. */
fun RepositoryException.wordsOr(fallback: String): String = if (problem == Problem.Unavailable) problem.ownerWords else fallback

/**
 * Everything the screens need from Renoa: [RcpAgentRepository] in the app, [FakeAgentRepository]
 * in tests.
 */
interface AgentRepository {
    val link: StateFlow<Link>

    val agents: StateFlow<List<AgentSummary>>

    /** The note at the top of Home, or null when there is nothing to say. */
    val briefing: StateFlow<Briefing?>

    fun conversation(agentId: String): Flow<Conversation>

    /** Returns once the message is kept. The reply arrives later through [conversation]. */
    suspend fun send(agentId: String, text: String)

    /** Answers the agent's question with one of its choices. The answer is kept as the owner's message. */
    suspend fun answer(agentId: String, choice: String)

    /** The owner has read what the agent said. */
    suspend fun markSeen(agentId: String)

    /** Stops what the agent is doing now. It stays stopped. */
    suspend fun stop(agentId: String)

    suspend fun profile(agentId: String): AgentProfile

    /** The abilities a new agent starts with, so the create screen can show them before the agent exists. */
    suspend fun starterAbilities(): List<AbilitySwitch>

    suspend fun create(request: CreateAgent): AgentProfile

    suspend fun edit(agentId: String, edit: EditProfile): AgentProfile

    suspend fun setAbility(agentId: String, abilityId: String, lit: Boolean): AbilitySwitch

    /** Whether Renoa has made any agent for the owner yet. */
    suspend fun hasAgents(): Boolean

    /** The owner's profile, which every agent that talks to them reads. */
    suspend fun ownerProfile(): OwnerProfile

    /**
     * Replaces the owner's profile from the revision it was read at, so a change made since is a
     * [Problem.Conflict]. Saving the same content again, as after a lost answer, succeeds.
     */
    suspend fun saveOwnerProfile(expectedRevision: String, content: String): OwnerProfile

    /** What the agents do on a schedule, as Renoa last told it. */
    val routines: StateFlow<List<Routine>>

    /**
     * Turns a routine on or off. [operationId] is chosen once for the change and goes to Renoa with
     * it, which applies a repeat of the same operation once. A routine changed elsewhere since it was
     * read is a [Problem.Conflict].
     */
    suspend fun setRoutine(routineId: String, on: Boolean, operationId: String)
}

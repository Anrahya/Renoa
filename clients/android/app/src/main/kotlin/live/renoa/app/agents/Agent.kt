package live.renoa.app.agents

import kotlinx.serialization.Serializable

/**
 * What an agent is doing, as the owner sees it. This is the one place the five states live:
 * the badge, Home and Chat all read it, and none of them keeps its own list.
 */
@Serializable
enum class AgentState {
    Resting,
    Working,
    Wants,
    Stuck,
    Asleep,
    ;

    /** Where an agent sits on Home: whoever needs the owner first, then the stuck, the busy, the quiet and the sleeping. */
    val homeRank: Int
        get() =
            when (this) {
                Wants -> 0
                Stuck -> 1
                Working -> 2
                Resting -> 3
                Asleep -> 4
            }

    /** Whether the owner is the one who has to act: the agent asked something, or cannot go on without them. */
    val needsOwner: Boolean
        get() = homeRank <= Stuck.homeRank
}

/** An agent as the roster shows it. */
@Serializable
data class AgentSummary(
    val id: String,
    val name: String,
    /** What the agent is for, in the owner's words. */
    val purpose: String,
    val state: AgentState,
    val glance: Glance = Glance(),
    /**
     * Whether this is the owner's main agent, the one they talk to and that hands work to the others.
     * The Host names it: the first agent made, until the owner names another. Null until the Host
     * has said which agent is main.
     */
    val primary: Boolean? = null,
)

/** A question only the owner can answer, with the answers the agent offers. */
@Serializable
data class Ask(val question: String, val choices: List<String>)

/**
 * What Home shows about an agent at a glance. [line] is the last thing it said, or when it wakes
 * if it is asleep; [doing] is a plain line about what it is doing while it works.
 */
@Serializable
data class Glance(
    val line: String? = null,
    val time: String? = null,
    val unread: Boolean = false,
    val ask: Ask? = null,
    val doing: String? = null,
    /** The agent it is working with right now, by id. */
    val with: String? = null,
)

/** The note at the top of Home, in the voice of the agent who keeps an eye on everything. */
data class Briefing(val agentId: String, val text: String)

enum class Speaker { Owner, Agent }

/**
 * [notSent]: the app stopped sending the owner's message because Renoa could not take it.
 * [AgentRepository.retry] sends it again and [AgentRepository.discard] deletes it.
 */
data class ChatMessage(val id: String, val speaker: Speaker, val text: String, val notSent: Boolean = false)

/**
 * A reply still arriving. It is never stored. It already has the id its whole [ChatMessage] will
 * take, so the screen keeps one piece of text from the first word to the last.
 */
data class StreamingReply(val id: String, val text: String)

/**
 * One endless chat. Whether the agent is working is the agent's [AgentState], not a second
 * flag here.
 */
data class Conversation(val messages: List<ChatMessage> = emptyList(), val streaming: StreamingReply? = null)

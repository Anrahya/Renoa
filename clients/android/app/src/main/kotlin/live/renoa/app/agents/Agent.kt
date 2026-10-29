package live.renoa.app.agents

/**
 * What an agent is doing, as the owner sees it. This is the one place the five states live:
 * the badge, Home and Chat all read it, and none of them keeps its own list.
 */
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
data class AgentSummary(
    val id: String,
    val name: String,
    /** What the agent is for, in the owner's words. */
    val purpose: String,
    val state: AgentState,
    val glance: Glance = Glance(),
    /** The owner's main agent: the first one made, the one they talk to and that hands work to the others. */
    val primary: Boolean = false,
)

/** A question only the owner can answer, with the answers the agent offers. */
data class Ask(val question: String, val choices: List<String>)

/**
 * What Home shows about an agent at a glance. [line] is the last thing it said, or when it wakes
 * if it is asleep; [doing] is a plain line about what it is doing while it works.
 */
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

data class ChatMessage(val id: String, val speaker: Speaker, val text: String)

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

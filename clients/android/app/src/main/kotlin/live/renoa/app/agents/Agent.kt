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

    /** Busy states belong on Home's stage; the rest sit in the hive. */
    val onStage: Boolean
        get() =
            when (this) {
                Wants, Stuck, Working -> true
                Resting, Asleep -> false
            }

    /** Lower comes first when more agents want the stage than it has room for. */
    val stageRank: Int
        get() =
            when (this) {
                Wants -> 0
                Stuck -> 1
                Working -> 2
                Resting -> 3
                Asleep -> 4
            }
}

/** An agent as the roster shows it. */
data class AgentSummary(
    val id: String,
    val name: String,
    /** What the agent is for, in the owner's words. */
    val purpose: String,
    val state: AgentState,
)

enum class Speaker { Owner, Agent }

data class ChatMessage(val id: String, val speaker: Speaker, val text: String)

/**
 * One endless chat. [streaming] is text still arriving: it is never stored, and it is replaced
 * by a [ChatMessage] when the reply is whole. Whether the agent is working is the agent's
 * [AgentState], not a second flag here.
 */
data class Conversation(val messages: List<ChatMessage> = emptyList(), val streaming: String? = null)

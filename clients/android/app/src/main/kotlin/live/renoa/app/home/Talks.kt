package live.renoa.app.home

import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.Ask
import live.renoa.app.agents.Feed
import live.renoa.app.agents.Note
import live.renoa.app.agents.NoteKind
import live.renoa.app.badge.mainAgent
import java.time.Instant

/**
 * One agent's row on Home: what it has to say, newest first. [line] is its question, else its
 * newest note, else what it is doing or last said; [notes] are the feed's notes it shows, which
 * opening the row puts away; [problem] is its newest failed routine, which can run again.
 */
data class Talk(
    val agent: AgentSummary,
    val line: String?,
    val at: Instant?,
    val ask: Ask?,
    val notes: List<Note>,
    val problem: Note?,
) {
    /** The owner is the one who has to act: a question, or an agent that cannot go on. */
    val needsOwner: Boolean get() = ask != null || agent.state.needsOwner
}

/**
 * Home's list. [main] is the main agent, pinned on top with what needs the owner or the day's
 * line; [talking] are the others with something to say, whoever needs the owner first, then the
 * newest; [quiet] are the rest, one line each, in Home's order.
 */
data class TalkList(val main: Talk?, val talking: List<Talk>, val quiet: List<Talk>)

fun talks(roster: List<AgentSummary>, feed: Feed): TalkList {
    val main = mainAgent(roster)
    val all = homeOrder(roster).map { talk(it, feed, today = feed.today.takeIf { _ -> it.id == main?.id }) }
    val (talking, quiet) = all.filter { it.agent.id != main?.id }.partition { it.saysSomething() }
    return TalkList(
        all.firstOrNull { it.agent.id == main?.id },
        talking.sortedWith(compareBy<Talk> { !it.needsOwner }.thenByDescending { it.at ?: Instant.MIN }),
        quiet,
    )
}

private fun Talk.saysSomething(): Boolean = needsOwner || notes.isNotEmpty() || agent.state == AgentState.Working || agent.glance.unread

private fun talk(agent: AgentSummary, feed: Feed, today: String?): Talk {
    val mine = feed.notes.filter { it.agentId == agent.id }
    val notes = mine.filter { it.kind != NoteKind.Problem }
    val problem = mine.firstOrNull { it.kind == NoteKind.Problem }
    val glance = agent.glance
    val line =
        glance.ask?.question
            ?: notes.firstOrNull()?.body
            ?: today
            ?: (if (agent.state == AgentState.Working) glance.doing?.let { "$it…" } ?: "Working on it…" else null)
            ?: problem?.title
            ?: glance.line
    val at = listOfNotNull(glance.ask?.at, notes.firstOrNull()?.at).maxOrNull()
    return Talk(agent, line, at, glance.ask, notes, problem)
}

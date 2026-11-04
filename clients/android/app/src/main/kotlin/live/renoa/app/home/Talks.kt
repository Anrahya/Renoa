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
 * One agent's row on Home. [line] is its question, else its newest note, else its newest failed
 * routine, else what it is doing or last said; [note] is the note the line shows, which opening the
 * row puts away, so a note the row does not show stays for later; [problem] is its newest failed
 * routine, which can run again; [partner] is the agent it is working with, if any.
 */
data class Talk(
    val agent: AgentSummary,
    val line: String?,
    val at: Instant?,
    val ask: Ask?,
    val note: Note?,
    val problem: Note?,
    val partner: AgentSummary? = null,
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
    val all = homeOrder(roster).map { agent -> talk(agent, feed, today = feed.today.takeIf { agent.id == main?.id }, partner = roster.firstOrNull { it.id == agent.glance.with }) }
    val (talking, quiet) = all.filter { it.agent.id != main?.id }.partition { it.saysSomething() }
    return TalkList(
        all.firstOrNull { it.agent.id == main?.id },
        talking.sortedWith(compareBy<Talk> { !it.needsOwner }.thenByDescending { it.at ?: Instant.MIN }),
        quiet,
    )
}

private fun Talk.saysSomething(): Boolean = needsOwner || note != null || agent.state == AgentState.Working || agent.glance.unread

private fun talk(agent: AgentSummary, feed: Feed, today: String?, partner: AgentSummary?): Talk {
    val mine = feed.notes.filter { it.agentId == agent.id }
    val notes = mine.filter { it.kind != NoteKind.Problem }
    val problem = mine.firstOrNull { it.kind == NoteKind.Problem }
    val glance = agent.glance
    val note = notes.firstOrNull()?.takeIf { glance.ask == null }
    val line =
        glance.ask?.question
            ?: note?.words
            ?: problem?.title
            ?: today
            ?: (if (agent.state == AgentState.Working) working(glance.doing, partner) else null)
            ?: glance.line
    val at = listOfNotNull(glance.ask?.at, notes.firstOrNull()?.at).maxOrNull()
    return Talk(agent, line, at, glance.ask, note, problem, partner)
}

/** What a working agent's line says: who it works with and what it is doing. */
private fun working(doing: String?, partner: AgentSummary?): String =
    listOfNotNull(partner?.let { "With ${it.name}" }, doing?.let { "$it…" }).joinToString(" · ").ifEmpty { "Working on it…" }

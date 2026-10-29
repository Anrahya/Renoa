package live.renoa.app.home

import live.renoa.app.agents.AgentState
import live.renoa.app.agents.AgentSummary
import live.renoa.app.agents.Ask
import live.renoa.app.agents.Briefing
import live.renoa.app.agents.listed

/** What a handover is, and the verb its card puts after the agent's name. */
enum class HandoverKind(val verb: String) {
    Needs("asks"),
    Says("says"),
    Working("is on it"),
}

/**
 * One thing Home hands the owner, from one agent. [key] stays the same while it is the same
 * thing, so the card keeps its place when the list around it changes.
 */
data class Handover(val key: String, val agent: AgentSummary, val kind: HandoverKind, val text: String, val time: String? = null, val ask: Ask? = null)

/**
 * Everything Home could hand the owner, most important first: what needs them (a question, or an
 * agent that cannot go on), then what is for them (the main agent's note, then anything unread),
 * then who is at work. Within a kind, agents keep Home's order ([homeOrder]). Each agent has one
 * card, its most important; the main agent's note is the one card beside it.
 */
fun handovers(people: List<AgentSummary>, briefing: Briefing?): List<Handover> {
    val ordered = homeOrder(people)
    val needs =
        ordered.filter { it.state.needsOwner }.map {
            Handover("${it.id}/needs", it, HandoverKind.Needs, it.glance.ask?.question ?: it.glance.line ?: "${it.name} needs you.", it.glance.time, it.glance.ask)
        }
    val note = briefing?.let { b -> people.firstOrNull { it.id == b.agentId }?.let { Handover("${it.id}/note", it, HandoverKind.Says, b.text) } }
    val unread =
        ordered.filter { !it.state.needsOwner && it.glance.unread }.mapNotNull { agent ->
            agent.glance.line?.let { Handover("${agent.id}/says", agent, HandoverKind.Says, it, agent.glance.time) }
        }
    val working = ordered.filter { it.state == AgentState.Working }.map { Handover("${it.id}/working", it, HandoverKind.Working, it.glance.doing ?: "Working on it") }
    return needs + listOfNotNull(note) + (unread + working).distinctBy { it.agent.id }
}

/** The first agent working with another, and that other: the pair whose wire Home lights. */
fun talking(people: List<AgentSummary>): Pair<AgentSummary, AgentSummary>? =
    people.firstNotNullOfOrNull { agent ->
        if (agent.state != AgentState.Working) null else agent.glance.with?.let { id -> people.firstOrNull { it.id == id } }?.let { agent to it }
    }

/** Whoever said something the owner has not read, in Home's order. */
fun freshFrom(people: List<AgentSummary>): List<AgentSummary> = homeOrder(people).filter { it.glance.unread }

/** The dock's first line: "New from Tifa and Optimus", "New from Tifa, Optimus and 2 more", or "Chats". */
fun dockTitle(fresh: List<AgentSummary>): String =
    if (fresh.isEmpty()) "Chats" else "New from " + listed(fresh.take(2).map { it.name } + listOfNotNull((fresh.size - 2).takeIf { it > 0 }?.let { "$it more" }))

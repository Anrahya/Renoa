import type { Agent, HostSnapshot } from "./host-contract";
import { agentActivity, currentReviews, sessionUnfinished } from "./host-presentation";

/** Project only recorded relationships. Session positions cannot order separate sessions. */
export function agentOverview(host: HostSnapshot, agent: Agent) {
  const connections = host.connections.filter(connection => connection.selected_by_agents.includes(agent.id));
  const routines = host.routines.filter(routine => routine.agent_id === agent.id);
  const sessions = host.sessions.filter(session => session.agent_id === agent.id);
  const reviews = host.reviews.filter(review => review.agent_id === agent.id);
  const repositories = host.review_repositories.filter(repository => repository.policy.agent_id === agent.id);
  const scheduled = routines.filter(routine => routine.enabled).sort((a, b) => a.next_due_ms - b.next_due_ms || a.id.localeCompare(b.id));
  return { connections, routines, sessions, reviews, repositories, scheduled,
    paused: routines.filter(routine => !routine.enabled),
    activity: agentActivity(host, agent),
    unfinishedSessions: sessions.filter(sessionUnfinished).length,
    unavailableSessions: sessions.filter(session => session.observation === "unavailable").length,
    latestReviews: currentReviews(reviews),
    // The review catalog has admission order, not completion order.
    latestAdmission: reviews.at(-1),
  };
}

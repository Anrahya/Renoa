import type { Agent, HostSnapshot } from "./host-contract";
import { agentActivity, sessionUnfinished } from "./host-presentation";

/** Project only recorded relationships. Session positions cannot order separate sessions. */
export function agentOverview(host: HostSnapshot, agent: Agent) {
  const connections = host.connections.filter(connection => connection.selected_by_agents.includes(agent.id));
  const automations = host.automations.filter(automation => automation.agent_id === agent.id);
  const sessions = host.sessions.filter(session => session.agent_id === agent.id);
  const scheduled = automations.filter(automation => automation.enabled).sort((a, b) => a.next_due_ms - b.next_due_ms || a.id.localeCompare(b.id));
  return { connections, automations, sessions, scheduled,
    paused: automations.filter(automation => !automation.enabled),
    activity: agentActivity(host, agent),
    unfinishedSessions: sessions.filter(sessionUnfinished).length,
    unavailableSessions: sessions.filter(session => session.observation === "unavailable").length,
  };
}

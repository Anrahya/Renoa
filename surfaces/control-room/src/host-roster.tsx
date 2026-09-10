import { ArrowRight } from "@phosphor-icons/react";
import type { Agent, HostSnapshot } from "./host-contract";
import { agentActivity, agentHref, displayName, isEarlier } from "./host-presentation";

export function AgentRoster({ host }: { host: HostSnapshot }) {
  const agents = host.agents.filter(a => !isEarlier(a));
  return <div className="host-system">
    <div className="host-agent-roster" aria-label="Your agents">
      {agents.map(agent => <AgentEntry key={agent.id} {...{ host, agent }} />)}
      {!agents.length && <p className="host-empty">No named agents yet. Existing identities are available under Agents.</p>}
    </div>
    <a href="#library" className="host-common-ground"><span className="host-ownership-mark" aria-hidden="true" />
      <span>One shared Host</span><span className="host-secondary">{host.connections.length} connections · {host.plugins.length} plugin revisions</span><ArrowRight size={18} aria-hidden="true" />
    </a>
  </div>;
}
export function AgentEntry({ host, agent, earlier = false }: { host: HostSnapshot; agent: Agent; earlier?: boolean }) {
  const activity = agentActivity(host, agent);
  const sessions = host.sessions.filter(s => s.agent_id === agent.id).length;
  const repositories = host.review_repositories.filter(r => r.policy.agent_id === agent.id).length;
  const connections = host.connections.filter(c => c.selected_by_profiles.includes(agent.profile)).length;
  return <a href={agentHref(agent.id)} className={earlier ? "host-earlier-agent" : "host-agent-entry"}>
    <span className="host-agent-name">{displayName(agent.name)}<ArrowRight size={22} aria-hidden="true" /></span>
    {earlier && <code>{agent.id.slice(0, 8)}</code>}
    <span className="host-agent-meta">{repositories ? `${repositories} review ${repositories === 1 ? "repository" : "repositories"}` : `${sessions} ${sessions === 1 ? "session" : "sessions"}`} · {connections} connections</span>
    <span className={`host-activity host-activity-${activity.tone}`}>{activity.label}</span>
  </a>;
}

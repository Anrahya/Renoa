import { ArrowRight } from "@phosphor-icons/react";
import { useState } from "react";
import type { Agent, HostSnapshot } from "./host-contract";
import { agentActivity, agentHref, displayName, isEarlier } from "./host-presentation";
import { AgentAvatar } from "./host-avatar";
import { findAgents } from "./host-system-model";

export function AgentRoster({ host }: { host: HostSnapshot }) {
  const [query, setQuery] = useState("");
  const agents = findAgents(host.agents.filter(a => !isEarlier(a)), query);
  return <div className="host-system host-directory">
    <label className="host-map-search"><span className="sr-only">Find an agent</span><input type="search" value={query} onChange={event => setQuery(event.target.value)} placeholder="Find an agent…" /></label>
    <div className="host-agent-roster" aria-label="Your agents">
      {agents.map(agent => <AgentEntry key={agent.id} {...{ host, agent }} />)}
      {!agents.length && <p className="host-empty">{query.trim() ? "No matching agents." : "No named agents yet. Earlier identities are listed below."}</p>}
    </div>
  </div>;
}
export function AgentEntry({ host, agent, earlier = false }: { host: HostSnapshot; agent: Agent; earlier?: boolean }) {
  const activity = agentActivity(host, agent);
  const sessions = host.sessions.filter(s => s.agent_id === agent.id).length;
  const repositories = host.review_repositories.filter(r => r.policy.agent_id === agent.id).length;
  const connections = host.connections.filter(c => c.selected_by_profiles.includes(agent.profile)).length;
  return <a href={agentHref(agent.id)} className={earlier ? "host-earlier-agent" : "host-agent-entry"}>
    <span className="host-agent-name"><span className="host-roster-identity"><AgentAvatar agentId={agent.id} name={agent.name} github={repositories > 0} />{displayName(agent.name)}</span><ArrowRight size={22} aria-hidden="true" /></span>
    {earlier && <code>{agent.id.slice(0, 8)}</code>}
    <span className="host-agent-meta">{repositories ? `${repositories} review ${repositories === 1 ? "repository" : "repositories"}` : `${sessions} ${sessions === 1 ? "session" : "sessions"}`} · {connections} connections</span>
    <span className={`host-activity host-activity-${activity.tone}`}>{activity.label}</span>
  </a>;
}

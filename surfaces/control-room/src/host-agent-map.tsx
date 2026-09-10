import { useEffect, useRef, useState } from "react";
import { ArrowRight, CaretRight, ChatCircle, Clock, GitBranch, GithubLogo, MagnifyingGlass, PlugsConnected, Stack, WarningCircle } from "@phosphor-icons/react";
import type { Agent, HostSnapshot } from "./host-contract";
import { agentActivity, agentHref, displayName, isEarlier } from "./host-presentation";
import { agentAncestors, agentLineage, findAgents } from "./host-lineage";
import "./styles/host-map.css";

export function AgentMap({ host }: { host: HostSnapshot }) {
  const [query, setQuery] = useState("");
  const [includeEarlier, setIncludeEarlier] = useState(false);
  const agents = host.agents.filter(agent => includeEarlier || !isEarlier(agent));
  const lineage = agentLineage(agents);
  const [selection, setSelection] = useState<string | null | undefined>(undefined);
  // A single recorded creator can lead the first view; no identity is hardcoded.
  const initial = lineage.roots.length === 1 && lineage.children.has(lineage.roots[0]!.id) ? lineage.roots[0]!.id : null;
  const focus = agents.find(agent => agent.id === (selection === undefined ? initial : selection));
  const children = focus ? lineage.children.get(focus.id) ?? [] : lineage.roots;
  const ancestors = focus ? agentAncestors(lineage, focus.id) : [];
  const results = findAgents(agents, query);
  const earlier = host.agents.filter(isEarlier).length;
  const currentCrumb = useRef<HTMLButtonElement>(null);
  const scrollArea = useRef<HTMLDivElement>(null);
  const restoreGroupFocus = useRef(false);
  useEffect(() => {
    if (!restoreGroupFocus.current) return;
    restoreGroupFocus.current = false;
    currentCrumb.current?.focus({ preventScroll: true });
    scrollArea.current?.scrollTo(0, 0);
  }, [selection, query]);
  function openGroup(id: string | null) {
    restoreGroupFocus.current = true;
    setSelection(id); setQuery("");
  }
  return <section className="host-map" aria-label="Agent relationships">
    <div className="host-map-toolbar">
      <nav className="host-map-path" aria-label="Agent groups">
        <button ref={!focus ? currentCrumb : undefined} onClick={() => openGroup(null)} aria-current={!focus ? "location" : undefined}>Host <span>{agents.length}</span></button>
        {ancestors.map(agent => <span key={agent.id}><CaretRight size={14} aria-hidden="true" /><button onClick={() => openGroup(agent.id)}>{displayName(agent.name)}</button></span>)}
        {focus && <span><CaretRight size={14} aria-hidden="true" /><button ref={currentCrumb} aria-current="location" onClick={() => openGroup(focus.id)}>{displayName(focus.name)}</button></span>}
      </nav>
      <label className="host-map-search"><MagnifyingGlass size={18} aria-hidden="true" /><span className="sr-only">Find an agent</span>
        <input type="search" value={query} onChange={event => setQuery(event.target.value)} placeholder="Find an agent…" /></label>
    </div>
    {query.trim() ? <div className="host-map-results">
      <p className="host-caption" role="status">{results.length} {results.length === 1 ? "agent" : "agents"} found</p>
      {results.map(agent => <div className="host-map-result" key={agent.id}><AgentNode host={host} agent={agent} />
        {agent.created_by && <span className="host-caption">Created by {displayName(host.agents.find(a => a.id === agent.created_by)?.name ?? "an unavailable agent")}</span>}
        {!!lineage.children.get(agent.id)?.length && <button className="host-group-link" onClick={() => openGroup(agent.id)}>View created agents <ArrowRight size={16} aria-hidden="true" /></button>}
      </div>)}
      {!results.length && <p className="host-empty">No matching agents. Try another name or include earlier identities.</p>}
    </div> : <div ref={scrollArea} id="host-map-scroll" className="host-map-scroll" role="region" aria-label="Agent creation map" tabIndex={0}>
      <div className={`host-branch ${children.length ? children.length === 1 ? "host-branch-single" : "" : "host-branch-empty"}`}>
        <div className="host-branch-origin">
          {focus ? <><AgentNode host={host} agent={focus} central /><span className="host-map-relationship"><GitBranch size={16} aria-hidden="true" /> Created {children.length} {children.length === 1 ? "agent" : "agents"}</span></>
            : <div className="host-origin"><span className="host-origin-mark" aria-hidden="true">r.</span><strong>Renoa Host</strong><span>Owns all {host.agents.length} agents</span></div>}
        </div>
        {!!children.length && <ul className="host-satellites" aria-label={focus ? `Agents created by ${displayName(focus.name)}` : "Agent groups"}>
          {children.map(agent => <li className="host-satellite" key={agent.id}><AgentNode host={host} agent={agent} />
            {lineage.detached.has(agent.id) && <span className="host-caption">Creator relationship unavailable</span>}
            {!!lineage.children.get(agent.id)?.length && <button className="host-group-link" onClick={() => openGroup(agent.id)}>
              <GitBranch size={16} aria-hidden="true" /> {lineage.children.get(agent.id)!.length} created <ArrowRight size={16} aria-hidden="true" /></button>}
          </li>)}
        </ul>}
        {!children.length && <p className="host-empty">{focus ? "No agents created by this agent." : "No agents to display."}</p>}
      </div>
    </div>}
    <div className="host-map-key"><p><GitBranch size={16} aria-hidden="true" /> {focus ? "Lines show who created whom. All agents belong to the Host." : "All agents belong to the Host. Expand a group to see its created agents."}</p>
      {!!earlier && <label><input type="checkbox" checked={includeEarlier} onChange={event => { setIncludeEarlier(event.target.checked); setSelection(null); }} /> Include {earlier} earlier identities</label>}</div>
    <a href="#library" className="host-map-library"><Stack size={20} aria-hidden="true" /><strong>Shared library</strong>
      <span>{host.connections.length} connections <span aria-hidden="true">·</span> {host.plugins.length} plugin revisions <span aria-hidden="true">·</span> {host.skills.length} skills</span><ArrowRight size={18} aria-hidden="true" /></a>
  </section>;
}

function AgentNode({ host, agent, central = false }: { host: HostSnapshot; agent: Agent; central?: boolean }) {
  const activity = agentActivity(host, agent);
  const review = host.review_repositories.some(repository => repository.policy.agent_id === agent.id);
  const sessions = host.sessions.filter(session => session.agent_id === agent.id).length;
  const connections = host.connections.filter(connection => connection.selected_by_profiles.includes(agent.profile)).length;
  const schedules = host.routines.filter(routine => routine.agent_id === agent.id && routine.enabled).length;
  const initials = displayName(agent.name).split(/\s+/).map(part => part[0]).join("").slice(0, 2);
  return <div className={`host-map-node ${central ? "host-map-central" : ""} host-map-${activity.tone}`}>
    <a href={agentHref(agent.id)} className="host-node-open">
      <span className="host-node-orb" aria-hidden="true">{review ? <GithubLogo size={central ? 38 : 30} /> : initials}
        {activity.tone === "attention" && <span className="host-node-alert"><WarningCircle size={20} weight="fill" /></span>}</span>
      <span className="host-node-copy"><strong>{displayName(agent.name)}<ArrowRight size={18} aria-hidden="true" /></strong>
        <span className="host-node-role">{review ? "GitHub reviews" : "Agent"}{isEarlier(agent) && <> · {agent.id.slice(0, 8)}</>}</span>
        <span className={`host-node-state host-activity-${activity.tone}`} title={activity.label}>{activity.tone === "quiet" ? "No pending work" : activity.label}</span>
      </span>
    </a>
    <div className="host-node-resources">
      <a href={agentHref(agent.id, "connections")} aria-label={`${displayName(agent.name)}: ${connections} MCP connections`} title="Selected MCP connections"><PlugsConnected size={16} aria-hidden="true" />{connections}</a>
      <a href={agentHref(agent.id)} aria-label={`${displayName(agent.name)}: ${sessions} sessions`} title="Recorded sessions"><ChatCircle size={16} aria-hidden="true" />{sessions}</a>
      <a href={agentHref(agent.id, "automations")} aria-label={`${displayName(agent.name)}: ${schedules} enabled schedules`} title="Enabled schedules"><Clock size={16} aria-hidden="true" />{schedules}</a>
    </div>
  </div>;
}

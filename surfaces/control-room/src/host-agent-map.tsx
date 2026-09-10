import { useRef, useState, type CSSProperties } from "react";
import { ArrowUpRight, Clock, GithubLogo, Hourglass, MagnifyingGlass, Pause, Play, WarningCircle } from "@phosphor-icons/react";
import type { Agent, HostSnapshot, Routine } from "./host-contract";
import { agentActivity, agentHref, displayName, isEarlier, scheduleText } from "./host-presentation";
import { findAgents, scheduleCountdown } from "./host-system-model";
import { useSystemMotion } from "./host-system-motion";
import { SystemConnections } from "./host-system-connections";
import "./styles/host-map.css";

export function AgentMap({ host, live = false, receivedAt = null }: { host: HostSnapshot; live?: boolean; receivedAt?: number | null }) {
  const [query, setQuery] = useState("");
  const [includeEarlier, setIncludeEarlier] = useState(false);
  const [paused, setPaused] = useState(false);
  const viewport = useRef<HTMLDivElement>(null);
  const stage = useRef<HTMLDivElement>(null);
  const motion = useSystemMotion(viewport, host, live, receivedAt, paused);
  const agents = findAgents(host.agents.filter(agent => includeEarlier || !isEarlier(agent)), query);
  const earlier = host.agents.filter(isEarlier).length;
  const identity = agents.map(agent => `${agent.id}:${host.routines.filter(r => r.agent_id === agent.id).map(r => r.id).join(",")}`).join(";");
  return <section className="system-map" aria-label="Host system" data-moving={motion.moving}>
    <div className="system-toolbar">
      <label className="host-map-search"><MagnifyingGlass size={18} aria-hidden="true" /><span className="sr-only">Find an agent</span>
        <input type="search" value={query} onChange={event => setQuery(event.target.value)} placeholder="Find an agent…" /></label>
      <div className="system-toolbar-actions">{!!earlier && <label><input type="checkbox" checked={includeEarlier} onChange={event => setIncludeEarlier(event.target.checked)} /> Earlier identities <span>{earlier}</span></label>}
        {live && <button className="host-icon" aria-label={paused ? "Resume map motion and timers" : "Pause map motion and timers"} title={paused ? "Resume motion and timers" : "Pause motion and timers"} onClick={() => setPaused(!paused)}>
          {paused ? <Play size={18} aria-hidden="true" /> : <Pause size={18} aria-hidden="true" />}</button>}</div>
    </div>
    <div ref={viewport} className="system-viewport" tabIndex={0} role="region" aria-label="Host and persistent agents">
      <div ref={stage} className="system-stage">
        <SystemConnections {...{ stage, viewport, identity }} pulses={motion.pulses} />
        <div className="system-host"><div data-host-anchor className="system-host-orb"><span>r.</span><svg className="system-host-ring" viewBox="0 0 120 120" aria-hidden="true"><circle cx="60" cy="60" r="56" /><circle className="system-feed-dot" cx="60" cy="4" r="3" /></svg></div>
          <strong>Renoa Host</strong><span className={live ? "system-connected" : ""}>{live ? "Connected" : "Saved state"}</span>
        </div>
        <ul className="system-agents" aria-label="Host-owned agents">{agents.map(agent => <li className="system-agent" key={agent.id}>
          <SystemAgent host={host} agent={agent} pulse={motion.pulses.get(agent.id) ?? null} />
          <AgentSchedules routines={host.routines.filter(r => r.agent_id === agent.id)} now={motion.now} />
        </li>)}</ul>
        {!agents.length && <p className="system-empty" role="status">{query.trim() ? "No matching agents." : "No persistent agents recorded."}</p>}
      </div>
    </div>
    <div className="system-key"><span><span className="system-key-dot" /> No pending work</span><span><Hourglass size={14} aria-hidden="true" /> Unfinished</span><span><WarningCircle size={15} aria-hidden="true" /> Attention</span><span><Clock size={15} aria-hidden="true" /> Schedule</span>
      <details><summary>About this view</summary><p>Persistent agents belong directly to the Host. Schedules attach to their target agent. Creator history is available inside each agent.</p><p>Light sweeps mark newly received execution records, not worker heartbeats. Timers use the recorded due time; reaching zero does not confirm execution. Saved snapshots stay still.</p></details>
    </div>
  </section>;
}

function SystemAgent({ host, agent, pulse }: { host: HostSnapshot; agent: Agent; pulse: number | null }) {
  const state = agentActivity(host, agent);
  const review = host.review_repositories.some(repository => repository.policy.agent_id === agent.id);
  const initials = displayName(agent.name).split(/\s+/).map(part => part[0]).join("").slice(0, 2);
  return <a className={`system-agent-link system-state-${state.tone}`} href={agentHref(agent.id)} aria-label={`${displayName(agent.name)}: ${review ? "GitHub reviews. " : ""}${state.label}`} title={state.label}>
    <span className="system-agent-orb" data-agent-anchor={agent.id}><span aria-hidden="true">{review ? <GithubLogo size={30} /> : initials}</span>
      {pulse !== null && <svg key={pulse} className="system-activity-ring" viewBox="0 0 80 80" aria-hidden="true"><circle cx="40" cy="40" r="36" pathLength="1" /></svg>}
      <span className="system-agent-indicator" aria-hidden="true">{state.tone === "attention" ? <WarningCircle size={19} weight="fill" /> : state.tone === "pending" ? <Hourglass size={17} /> : <span />}</span>
    </span>
    <span className="system-agent-name"><strong>{displayName(agent.name)}</strong>{review && <small>GitHub reviews</small>}{isEarlier(agent) && <small>{agent.id.slice(0, 8)}</small>}</span><ArrowUpRight size={18} className="system-open-arrow" aria-hidden="true" />
    <span className="sr-only" role="status">{pulse !== null ? "New execution records received" : ""}</span>
  </a>;
}

function AgentSchedules({ routines, now }: { routines: Routine[]; now: number | null }) {
  if (!routines.length) return null;
  return <ul className="system-schedules" aria-label="Agent schedules">{routines.map(routine => {
    const value = scheduleCountdown(routine, now);
    const ticking = routine.enabled && !routine.pending_runs && now !== null && routine.next_due_ms > now;
    return <li key={routine.id}><a href={agentHref(routine.agent_id, "automations")} title={`${routine.name} · ${scheduleText(routine)}`} aria-label={`${routine.name}: ${value}`}>
      {ticking ? <span className="system-clock" style={{ "--clock-angle": `${Math.floor(now / 1000) * 6}deg` } as CSSProperties} aria-hidden="true" /> : routine.pending_runs ? <Hourglass size={15} aria-hidden="true" /> : routine.enabled ? <Clock size={15} aria-hidden="true" /> : <Pause size={15} aria-hidden="true" />}
      <span className="system-schedule-name">{routine.name}</span><span className="system-timer" aria-hidden="true">{value}</span>
    </a></li>;
  })}</ul>;
}

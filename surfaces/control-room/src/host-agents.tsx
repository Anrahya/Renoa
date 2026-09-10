import { ArrowLeft } from "@phosphor-icons/react";
import type { Agent, HostSnapshot } from "./host-contract";
import { RoutineRow, SessionRow, ReviewRow } from "./host-records";
import { ReviewPolicy, type Controls } from "./host-controls";
import { AgentRoster, AgentEntry } from "./host-roster";
import { ConnectionList } from "./host-library";
import { agentActivity, agentHref, currentReviews, displayName, isEarlier } from "./host-presentation";
import type { AgentSection, HostRoute } from "./host-navigation";

export function AgentsView({ host, route, controls }: { host: HostSnapshot; route: HostRoute; controls: Controls }) {
  const agent = host.agents.find(a => a.id === route.agent);
  if (agent) return <AgentView key={agent.id} {...{ host, agent, controls }} section={route.section} />;
  const earlier = host.agents.filter(isEarlier);
  return <main id="host-main" className="host-content"><h1>Your agents</h1>
    <p className="host-intro">Open an agent to inspect its work and configuration.</p>
    {route.agent && <p role="status" className="host-notice">That agent is not in this Host snapshot. Choose an available agent below.</p>}
    {!host.agents.length ? <p className="host-empty">This Host has no recorded agents yet.</p> : <AgentRoster host={host} />}
    {!!earlier.length && <details className="host-history host-section"><summary>Earlier identities <span>{earlier.length} records</span></summary>
      <p className="host-caption">Original profile-named identities, retained with their own records.</p>
      <div className="host-earlier-list">{earlier.map(agent => <AgentEntry key={agent.id} {...{ host, agent }} earlier />)}</div>
    </details>}
  </main>;
}
function AgentView({ host, agent, section, controls }: { host: HostSnapshot; agent: Agent; section: AgentSection; controls: Controls }) {
  const connections = host.connections.filter(c => c.selected_by_profiles.includes(agent.profile));
  const routines = host.routines.filter(r => r.agent_id === agent.id);
  const sessions = host.sessions.filter(s => s.agent_id === agent.id);
  const reviews = host.reviews.filter(r => r.agent_id === agent.id);
  const latest = currentReviews(reviews);
  const creator = host.agents.find(a => a.id === agent.created_by);
  const repositories = host.review_repositories.filter(r => r.policy.agent_id === agent.id);
  const activity = agentActivity(host, agent);
  const sections: { id: AgentSection; label: string; count: number }[] = [
    { id: "work", label: "Work", count: sessions.length + reviews.length },
    { id: "connections", label: "Connections", count: connections.length },
    { id: "automations", label: "Automations", count: routines.length },
    ...(repositories.length ? [{ id: "policy" as const, label: "Review policy", count: repositories.length }] : []),
  ];
  const active = section === "policy" && !repositories.length ? "work" : section;
  return <main id="host-main" className="host-content host-agent-detail">
    <a className="host-link host-back" href="#agents"><ArrowLeft size={16} aria-hidden="true" /> All agents</a>
    <div className="host-page-heading"><div><h1>{displayName(agent.name)}</h1><p className="host-intro">Host-owned agent{creator && <> · Created by <a className="host-link" href={agentHref(creator.id)}>{displayName(creator.name)}</a></>}</p></div>
      <span className={`host-activity host-activity-${activity.tone}`}>{activity.label}</span></div>
    <nav className="host-subnav" aria-label="Agent sections">{sections.map(item => <a key={item.id} href={agentHref(agent.id, item.id)} aria-current={active === item.id ? "page" : undefined}>{item.label}<span>{item.count}</span></a>)}</nav>
    <div className="host-agent-section" key={active}>
      {active === "work" && <>
        {!!reviews.length && <section><div className="host-section-heading"><h2>Latest reviews</h2><span className="host-caption">One attempt per pull request</span></div>
          {latest.map(review => <ReviewRow key={review.request_id} {...{ review, host }} preview={controls.preview} />)}
          {reviews.length > latest.length && <details className="host-history host-section"><summary>All review attempts <span>{reviews.length} records</span></summary>
            {[...reviews].reverse().map(review => <ReviewRow key={review.request_id} {...{ review, host }} preview={controls.preview} />)}</details>}
        </section>}
        {!!sessions.length && <section className={reviews.length ? "host-section" : ""}><div className="host-section-heading"><h2>Sessions</h2><span className="host-caption">Durable operation records</span></div>
          {sessions.map(session => <SessionRow key={session.id} {...{ session, host }} />)}</section>}
        {!reviews.length && !sessions.length && <p className="host-empty">No recorded work for this agent yet.</p>}
      </>}
      {active === "connections" && <><h2>Selected from the shared library</h2>
        <p className="host-intro">These connections are selected for this agent’s profile. An existing run may use an earlier configuration.</p>
        <ConnectionList host={host} connections={connections} />
        <a className="host-link host-section-link" href="#library">View the entire shared library</a></>}
      {active === "automations" && <><h2>Automations</h2><p className="host-intro">Schedules belong to the Host, wherever you talk to this agent.</p>
        {routines.length ? routines.map(routine => <RoutineRow key={routine.id} {...{ routine, controls }} />) : <p className="host-empty">This agent has no recorded automations.</p>}</>}
      {active === "policy" && <><h2>Review policy</h2><p className="host-intro">Choose which repository events start a review.</p>
        {repositories.map(repository => <ReviewPolicy key={repository.policy.repository_id} {...{ repository, controls }} />)}</>}
    </div>
    <details className="host-details host-identity"><summary>Agent identity</summary>
      <p>Agent <code>{agent.id}</code></p><p>Profile <code>{agent.profile}</code></p></details>
  </main>;
}

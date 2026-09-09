import type { Agent, HostSnapshot } from "./host-contract";
import { displayName, RoutineRow, SessionRow, ReviewRow } from "./host-work";
import { ReviewPolicy, type Controls } from "./host-controls";

export function AgentsView({ host, selected, openAgent, connections, controls }: {
  host: HostSnapshot; selected: string | null; openAgent: (id: string | null) => void; connections: () => void; controls: Controls;
}) {
  const agent = host.agents.find(a => a.id === selected);
  if (agent) return <AgentView {...{ host, agent, openAgent, connections, controls }} />;
  const current = host.agents.filter(a => !a.name.startsWith("renoa."));
  const earlier = host.agents.filter(a => a.name.startsWith("renoa."));
  function row(agent: Agent) {
    const sessions = host.sessions.filter(s => s.agent_id === agent.id).length;
    const routines = host.routines.filter(r => r.agent_id === agent.id).length;
    const repositories = host.review_repositories.filter(r => r.policy.agent_id === agent.id).length;
    return <button key={agent.id} className="host-agent-row" onClick={() => openAgent(agent.id)}>
      <span>{displayName(agent.name)}{agent.name.startsWith("renoa.") && <small>{agent.id.slice(0, 8)}</small>}</span>
      <span className="host-secondary">{repositories ? `${repositories} review ${repositories === 1 ? "repository" : "repositories"}` : `${sessions} ${sessions === 1 ? "session" : "sessions"}`} · {routines} {routines === 1 ? "automation" : "automations"}</span>
    </button>;
  }
  return <main className="host-content"><p className="host-kicker">Part of one system</p><h1>Your agents.</h1>
    <p className="host-intro">Their work stays with the Host, wherever you talk to them.</p>
    <section className="host-section">{current.map(row)}
      {host.agents.length === 0 && <p className="host-secondary">This Host has no recorded agents yet.</p>}</section>
    {earlier.length > 0 && <details className="host-details host-section"><summary>Earlier agent records · {earlier.length}</summary>
      <p className="host-caption">These identities use their original profile names. They remain separate records.</p>{earlier.map(row)}</details>}
  </main>;
}
function AgentView({ host, agent, openAgent, connections, controls }: {
  host: HostSnapshot; agent: Agent; openAgent: (id: string | null) => void; connections: () => void; controls: Controls;
}) {
  const selected = host.connections.filter(c => c.selected_by_profiles.includes(agent.profile));
  const routines = host.routines.filter(r => r.agent_id === agent.id);
  const sessions = host.sessions.filter(s => s.agent_id === agent.id);
  const reviews = host.reviews.filter(r => r.agent_id === agent.id).sort((a, b) => b.admitted_at_ms - a.admitted_at_ms);
  const creator = host.agents.find(a => a.id === agent.created_by);
  const repositories = host.review_repositories.filter(r => r.policy.agent_id === agent.id);
  const unfinishedReviews = reviews.filter(r => ["queued", "prepared"].includes(r.state)).length;
  return <main className="host-content host-agent-detail">
    <button className="host-link host-back" onClick={() => openAgent(null)}>All agents</button>
    <h1>{displayName(agent.name)}</h1><p className="host-subtitle">Host-owned agent{creator && ` · Created by ${displayName(creator.name)}`}</p>
    <p className="host-status-line">{unfinishedReviews ? `${unfinishedReviews} unfinished ${unfinishedReviews === 1 ? "review" : "reviews"} recorded.` : sessions.some(s => s.observation === "available" && s.active_operation)
      ? "Unfinished operations recorded." : sessions.some(s => s.observation === "unavailable")
        ? "Some session records are unavailable." : "No active session operation recorded."} <a className="host-link" href="#agent-runs">View runs</a></p>
    {repositories.length > 0 && <section className="host-section"><h2>Review policy</h2>
      {repositories.map(repository => <ReviewPolicy key={repository.policy.repository_id} {...{ repository, controls }} />)}</section>}
    <section className="host-section"><h2>Selected connections</h2>
      {selected.length ? <p className="host-capabilities">{selected.map((c, i) => <span key={c.id}>{i > 0 && " · "}<button className="host-link" onClick={connections}>{c.id}</button></span>)}</p>
        : <p className="host-secondary">No shared MCP connections are selected for this profile.</p>}
      <p className="host-caption">Selected from the Host’s shared library. An existing run may use an earlier configuration.</p>
      <button className="host-link" onClick={connections}>Inspect shared connections</button></section>
    <section className="host-section"><h2>Automations</h2>
      {routines.length ? routines.map(routine => <RoutineRow key={routine.id} routine={routine} controls={controls} />)
        : <p className="host-secondary">No automations recorded for this agent.</p>}</section>
    <section className="host-section" id="agent-runs"><h2>Recorded work</h2>
      {reviews.map(review => <ReviewRow key={review.request_id} {...{ review, host, openAgent }} preview={controls.preview} />)}
      {sessions.map(session => <SessionRow key={session.id} {...{ session, host, openAgent }} />)}
      {!reviews.length && !sessions.length && <p className="host-secondary">No recorded work yet.</p>}</section>
    <details className="host-details host-section"><summary>Agent identity</summary>
      <p className="host-caption">Agent <code>{agent.id}</code></p><p className="host-caption">Profile <code>{agent.profile}</code></p></details>
  </main>;
}
export function ConnectionsView({ host, openAgent }: { host: HostSnapshot; openAgent: (id: string) => void }) {
  return <main className="host-content"><p className="host-kicker">Available across your system</p><h1>Shared capabilities.</h1>
    <p className="host-intro">Install once. Select the pieces each agent needs.</p>
    <section className="host-section"><h2>Connections <small>{host.connections.length}</small></h2>
      {host.connections.map(connection => <details className="host-record" key={connection.id}>
        <summary><span>{connection.id}</span><span className="host-record-state">{connection.catalog_available ? `${connection.tool_count} catalogued tools` : "Catalog unavailable"}</span></summary>
        <div className="host-record-body"><p>Stored catalog only. Connection health has not been checked.</p>
          <p>Selected by {connection.selected_by_profiles.length} profiles</p>
          {connection.selected_by_profiles.map(profile => <p className="host-caption" key={profile}>
            {host.agents.filter(a => a.profile === profile).length ? host.agents.filter(a => a.profile === profile).map(agent =>
              <button key={agent.id} className="host-link host-owner-link" onClick={() => openAgent(agent.id)}>{displayName(agent.name)} <small>{agent.id.slice(0, 8)}</small></button>) : <code>{profile}</code>}</p>)}</div>
      </details>)}{!host.connections.length && <p className="host-secondary">No shared connections installed yet.</p>}</section>
    <section className="host-section"><h2>Plugins <small>{host.plugins.length}</small></h2>
      {host.plugins.map(plugin => <details className="host-record" key={plugin.digest}><summary><span>{plugin.name}</span>
        <span className="host-record-state">{plugin.version ?? "Installed revision"}</span></summary><div className="host-record-body"><code>{plugin.digest}</code></div></details>)}
      {!host.plugins.length && <p className="host-secondary">No plugins installed.</p>}</section>
    <section className="host-section"><h2>Recorded skills <small>{host.skills.length}</small></h2>
      <p className="host-caption">Stored revisions; these are not necessarily loaded in a session.</p>
      {host.skills.map(skill => <details className="host-record" key={skill.digest}><summary>{skill.name}</summary>
        <div className="host-record-body"><code>{skill.digest}</code></div></details>)}</section>
  </main>;
}

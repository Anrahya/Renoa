import { useState } from "react";
import type { Connection, HostSnapshot } from "./host-contract";
import { agentHref, connectionName, displayName, isEarlier } from "./host-presentation";

export function ConnectionList({ host, connections }: { host: HostSnapshot; connections: Connection[] }) {
  return <div className="host-connection-list">
    {connections.map(connection => {
      const selected = host.agents.filter(a => connection.selected_by_profiles.includes(a.profile));
      const named = selected.filter(a => !isEarlier(a));
      const earlier = selected.filter(isEarlier);
      return <details className="host-record" key={connection.id}>
        <summary><span>{connectionName(host, connection)}<small className="host-record-meta">{named.map(a => a.name).join(" · ") || "No named agent selects this connection"}</small></span>
          <span className={`host-record-state ${connection.catalog_available ? "" : "host-error"}`}>{connection.catalog_available ? `${connection.tool_count} tools` : "Catalog unavailable"}{" "}<small>Stored catalog</small></span></summary>
        <div className="host-record-body"><p className="host-caption">Tool catalog saved by the Host. Connection health has not been checked by this view.</p>
          <p>Selected by {connection.selected_by_profiles.length} profiles</p>
          <div className="host-inline-links">{named.map(agent => <a key={agent.id} className="host-link" href={agentHref(agent.id, "connections")}>{displayName(agent.name)}</a>)}</div>
          {!!earlier.length && <details className="host-details"><summary>{earlier.length} earlier agent identities</summary>
            <div className="host-inline-links">{earlier.map(agent => <a key={agent.id} className="host-link" href={agentHref(agent.id, "connections")}>{displayName(agent.name)} <code>{agent.id.slice(0, 8)}</code></a>)}</div></details>}
          <p className="host-caption">Connection <code>{connection.id}</code></p>
          <details className="host-details"><summary>Selected profile identities</summary>{connection.selected_by_profiles.map(profile => <p key={profile}><code>{profile}</code></p>)}</details>
        </div>
      </details>;
    })}
    {!connections.length && <p className="host-empty">No connections selected here.</p>}
  </div>;
}
export function ConnectionsView({ host }: { host: HostSnapshot }) {
  const [section, setSection] = useState("connections");
  return <main id="host-main" className="host-content"><h1>Shared library</h1>
    <p className="host-intro">The Host’s reusable pieces. Open a connection to see who selects it.</p>
    <nav className="host-subnav" aria-label="Library sections">{[{ id: "connections", label: "Connections", count: host.connections.length },
      { id: "plugins", label: "Plugins", count: host.plugins.length }, { id: "skills", label: "Skills", count: host.skills.length }].map(item =>
        <button key={item.id} aria-pressed={section === item.id} onClick={() => setSection(item.id)}>{item.label}<span>{item.count}</span></button>)}</nav>
    {section === "connections" && <><h2 className="sr-only">Connections</h2><ConnectionList host={host} connections={host.connections} /></>}
    {section === "plugins" && <><h2>Installed plugin revisions</h2><p className="host-intro">Packages recorded on this Host. Revisions keep their own identity.</p>
      {host.plugins.map(plugin => <details className="host-record" key={plugin.digest}><summary><span>{plugin.name}</span><span className="host-record-state">{plugin.version ?? "Stored revision"}</span></summary>
        <div className="host-record-body"><code>{plugin.digest}</code></div></details>)}
      {!host.plugins.length && <p className="host-empty">No plugins installed.</p>}</>}
    {section === "skills" && <><h2>Recorded skills</h2><p className="host-intro">Stored instructions. A recorded revision is not necessarily loaded in a session.</p>
      {host.skills.map(skill => <details className="host-record" key={skill.digest}><summary><span>{skill.name}</span><span className="host-record-state">Stored revision</span></summary>
        <div className="host-record-body"><code>{skill.digest}</code></div></details>)}
      {!host.skills.length && <p className="host-empty">No recorded skills.</p>}</>}
  </main>;
}

import { useState } from "react";
import { CaretRight, Cube, Key, MagnifyingGlass } from "@phosphor-icons/react";
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { NoResults, PageHeading, agentCount } from "./host-desk";
import { portraitForAgent } from "./host-identity";
import "./styles/host-connections-preview.css";
import type { HostSnapshot } from "./host-contract";
import { agentHref, connectionName, displayName } from "./host-presentation";

export function ConnectionsView({ host, tab = "plugins" }: { host: HostSnapshot; tab?: string }) {
  const [query, setQuery] = useState("");
  const section = tab === "accounts" ? "accounts" : "plugins";
  const term = query.trim().toLocaleLowerCase();
  const plugins = host.plugins.filter(plugin => `${plugin.name} ${plugin.version ?? ""}`.toLocaleLowerCase().includes(term));
  const skills = host.skills.filter(skill => skill.name.toLocaleLowerCase().includes(term));
  const connections = host.connections.filter(connection => `${connectionName(host, connection)} ${connection.id}`.toLocaleLowerCase().includes(term));
  return <main id="host-main" className="host-desk">
    <PageHeading title="Connections" description="The capabilities your agents share, and the access they use." />
    <Tabs value={section} onValueChange={value => { setQuery(""); window.location.hash = `#library/${value}`; }} className="gap-6">
      <div className="desk-toolbar"><TabsList variant="line" aria-label="Connections sections"><TabsTrigger value="plugins">Plugins <Badge variant="secondary">{host.plugins.length}</Badge></TabsTrigger><TabsTrigger value="accounts">Accounts <Badge variant="secondary">{host.connections.length}</Badge></TabsTrigger></TabsList>
        <div className="desk-search"><InputGroup><InputGroupAddon><MagnifyingGlass /></InputGroupAddon><InputGroupInput type="search" aria-label={`Search ${section}`} placeholder={section === "plugins" ? "Find a plugin or skill…" : "Find a connection…"} value={query} onChange={event => setQuery(event.target.value)} /></InputGroup></div>
      </div>
      <TabsContent value="plugins">
        <div className="library-explainer"><Cube size={20} /><p><strong>Your shared capability library</strong><span>Installed plugin revisions and skills. Agent selections and account access stay separate.</span></p></div>
        {plugins.map(plugin => <details className="desk-disclosure" key={plugin.digest}><summary><span className="desk-icon"><Cube /></span><span className="desk-disclosure-title"><strong>{plugin.name}</strong><small>{plugin.version ?? "Stored revision"}</small></span><span className="desk-status">Installed</span><CaretRight size={17} /></summary><div className="desk-disclosure-body"><p className="desk-note">This snapshot contains plugin metadata. Its nested capabilities and agent activations are not reported here.</p><code className="break-all text-xs">{plugin.digest}</code></div></details>)}
        {!plugins.length && <NoResults title={term ? "No matching plugins" : "No plugins installed"} clear={term ? () => setQuery("") : undefined} />}
        {(host.skills.length > 0 || !term) && <section className="mt-8"><div className="desk-section-heading"><h2>Recorded skills</h2><span className="desk-note">Stored instructions · assignment is not reported</span></div>{skills.map(skill => <details className="desk-disclosure" key={skill.digest}><summary><span className="desk-disclosure-title"><strong>{skill.name}</strong><small>Stored revision</small></span><CaretRight size={17} /></summary><div className="desk-disclosure-body"><code className="break-all text-xs">{skill.digest}</code></div></details>)}{!skills.length && <p className="desk-note">{term ? "No matching skills." : "No skills recorded yet."}</p>}</section>}
      </TabsContent>
      <TabsContent value="accounts"><p className="desk-note mb-4">Stored MCP catalogs and agent selections. Connection health has not been checked; account authorization is not reported.</p>
        {connections.map(connection => {
          const users = host.agents.filter(agent => connection.selected_by_agents.includes(agent.id));
          return <details className="desk-disclosure" key={connection.id}><summary><span className="desk-icon"><Key /></span><span className="desk-disclosure-title"><strong>{connectionName(host, connection)}</strong><small>{connection.catalog_available ? `${connection.tool_count} tools in stored catalog` : "Catalog unavailable"}</small></span><span className="library-member-count">{agentCount(connection.selected_by_agents.length)}</span><span className="desk-status">Authorization unknown</span><CaretRight size={17} /></summary><div className="desk-disclosure-body"><dl className="desk-facts"><div><dt>Catalog</dt><dd>{connection.catalog_available ? "Saved" : "Unavailable"}</dd></div><div><dt>Authorization</dt><dd>Not reported</dd></div><div><dt>Selected by</dt><dd>{agentCount(connection.selected_by_agents.length)}</dd></div></dl><div className="desk-agent-links">{users.map(agent => <a key={agent.id} className="desk-agent-link" href={agentHref(agent.id, "connections")}><img src={portraitForAgent(agent.id, agent.name)} alt="" />{displayName(agent.name)}</a>)}</div><code className="break-all text-xs">{connection.id}</code></div></details>;
        })}
        {!connections.length && <NoResults title={term ? "No matching connections" : "No connections installed"} clear={term ? () => setQuery("") : undefined} />}
      </TabsContent>
    </Tabs>
  </main>;
}

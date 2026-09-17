import { useState } from "react";
import { CaretRight, Cube, Key, MagnifyingGlass, Plugs, ArrowUpRight } from "@phosphor-icons/react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { Alert, AlertDescription } from "@/components/ui/alert";
import type { HostSnapshot } from "../host-contract";
import { capabilityPlugins, pluginCapabilities, type CapabilityPlugin } from "../agent-work-preview/configuration-model";
import { accountExamples, usePreviewConnections } from "../agent-work-preview/connection-state";
import { AgentLink, NoResults, PageHeading, agentCount, useDesignAgents, type DesignAgent } from "./shared";
import "../styles/host-connections-preview.css";

export function ConnectionsPreview({ host, tab }: { host: HostSnapshot; tab: string }) {
  const agents = useDesignAgents(host);
  const [query, setQuery] = useState("");
  const [notice, setNotice] = useState("");
  const { connected, setConnected } = usePreviewConnections();
  const term = query.trim().toLowerCase();
  const plugins = capabilityPlugins.filter(plugin => `${plugin.name} ${plugin.detail} ${plugin.groups.flatMap(group => [group.kind, group.via, ...group.items.map(item => item.name)]).join(" ")}`.toLowerCase().includes(term));
  const accounts = accountExamples.filter(account => `${account.name} ${account.service}`.toLowerCase().includes(term));
  return <main id="host-main" className="host-desk">
    <PageHeading title="Connections" description="The capabilities your agents share, and the accounts that power them." />
    <Tabs value={tab} onValueChange={value => { setQuery(""); window.location.hash = `#library/${value}`; }} className="gap-6">
      <div className="desk-toolbar"><TabsList variant="line" aria-label="Connections sections"><TabsTrigger value="plugins">Plugins <Badge variant="secondary">{capabilityPlugins.length}</Badge></TabsTrigger><TabsTrigger value="accounts">Accounts <Badge variant="secondary">{accountExamples.length}</Badge></TabsTrigger></TabsList>
        <div className="desk-search"><InputGroup><InputGroupAddon><MagnifyingGlass /></InputGroupAddon><InputGroupInput type="search" aria-label={`Search ${tab}`} placeholder={tab === "plugins" ? "Find plugins, tools or skills…" : "Find an account…"} value={query} onChange={event => setQuery(event.target.value)} /></InputGroup></div>
      </div>
      <TabsContent value="plugins">
        <div className="library-explainer"><Cube size={20} /><p><strong>One plugin. Any mix of capabilities.</strong><span>Tools, MCP servers, APIs and skills live inside a plugin. Each agent chooses what it can use.</span></p></div>
        {plugins.map(plugin => <Plugin key={plugin.id} plugin={plugin} agents={agents} searching={!!term} />)}
        {!plugins.length && <NoResults clear={() => setQuery("")} />}
      </TabsContent>
      <TabsContent value="accounts">
        <p className="desk-note mb-4">Accounts provide access. Connecting one doesn’t grant its tools to an agent.</p>
        {accounts.map(account => {
          const users = agents.filter(agent => account.id === "model" ? agent.configuration.model.startsWith("DeepSeek") : capabilityPlugins.some(plugin => plugin.connectionId === account.id && pluginCapabilities(plugin).some(capability => agent.configuration.capabilities.includes(capability.id))));
          const ready = connected[account.id] ?? false;
          return <details key={account.id} className="desk-disclosure"><summary><span className="desk-icon"><Key /></span><span className="desk-disclosure-title"><strong>{account.name}</strong><small>{account.service} · {account.method}</small></span><span className="desk-status" data-tone={ready ? "good" : "warning"}>{ready ? "Connected" : "Not connected"}</span><CaretRight size={17} /></summary>
            <div className="desk-disclosure-body"><dl className="desk-facts"><div><dt>Access</dt><dd>{account.scope}</dd></div><div><dt>Credentials</dt><dd>Held by the Host</dd></div><div><dt>Selected by</dt><dd>{agentCount(users.length)}</dd></div></dl>
              <div className="desk-agent-links">{users.map(agent => <AgentLink key={agent.id} agent={agent} configure />)}</div>
              <div className="library-account-action"><p className="desk-note">{ready ? "Disconnecting access leaves agent capability selections intact." : "Connecting restores access for agents that already select these capabilities."} This action only changes the preview.</p><Button variant="outline" onClick={() => { setConnected(account.id, !ready); setNotice(`${account.name} ${ready ? "disconnected" : "connected"} in this preview.`); }}>{ready ? "Disconnect" : "Connect"} in preview</Button></div>
            </div></details>;
        })}
        {!accounts.length && <NoResults clear={() => setQuery("")} />}
        {notice && <Alert className="mt-5"><AlertDescription role="status">{notice}</AlertDescription></Alert>}
      </TabsContent>
    </Tabs>
  </main>;
}
function Plugin({ plugin, agents, searching }: { plugin: CapabilityPlugin; agents: DesignAgent[]; searching: boolean }) {
  const ids = new Set(pluginCapabilities(plugin).map(item => item.id));
  const members = agents.filter(agent => agent.configuration.capabilities.some(id => ids.has(id)));
  const { connected } = usePreviewConnections();
  const ready = !plugin.connectionId || connected[plugin.connectionId];
  const account = accountExamples.find(item => item.id === plugin.connectionId);
  return <details className="desk-disclosure" open={searching || undefined}>
    <summary><span className="desk-icon"><Cube /></span><span className="desk-disclosure-title"><strong>{plugin.name}</strong><small>{pluginCapabilities(plugin).length} {pluginCapabilities(plugin).length === 1 ? "capability" : "capabilities"} · {plugin.groups.map(group => group.via ?? group.kind).join(" + ")}</small></span><span className="library-member-count">{agentCount(members.length)}</span><span className="desk-status" data-tone={ready ? "good" : "warning"}>{ready ? "Available" : "Needs account"}</span><CaretRight size={17} /></summary>
    <div className="desk-disclosure-body">
      <div className="library-bundle">{plugin.groups.map(group => <section key={`${group.kind}-${group.via}`}><div className="desk-section-heading"><h3>{group.kind}</h3>{group.via && <Badge variant="outline">{group.via}</Badge>}</div><ul>{group.items.map(item => <li key={item.id}><strong>{item.name}</strong><p>{item.detail}</p></li>)}</ul></section>)}</div>
      <div className="library-access"><div><h3>Used by</h3><p className="desk-note">{members.length ? "Open an agent to change its selection." : "No agents have selected this plugin yet."}</p><div className="desk-agent-links">{members.map(agent => <AgentLink key={agent.id} agent={agent} configure />)}</div>{!members.length && <Button asChild variant="outline" size="sm"><a href="#agents">Choose an agent<ArrowUpRight data-icon="inline-end" /></a></Button>}</div>
        <div><h3>Account access</h3>{account ? <a className="library-account-link" href="#library/accounts"><Plugs size={16} /><span>{account.name}<small>{ready ? "Connected" : "Connect to enable these tools"}</small></span><ArrowUpRight size={16} /></a> : <p className="desk-note">No account needed. Available directly in the agent’s environment.</p>}</div>
      </div>
    </div>
  </details>;
}

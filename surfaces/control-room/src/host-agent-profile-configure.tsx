import { useId, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import type { Agent, HostSnapshot } from "./host-contract";
import { agentHref, connectionName, displayName, isEarlier } from "./host-presentation";

export function ProfileConfigure({ host, agent }: { host: HostSnapshot; agent: Agent }) {
  const [query, setQuery] = useState("");
  const searchId = useId();
  const creator = host.agents.find(item => item.id === agent.created_by);
  const selected = host.connections.filter(connection => connection.selected_by_agents.includes(agent.id));
  const connections = [...host.connections].sort((a, b) => Number(b.selected_by_agents.includes(agent.id)) - Number(a.selected_by_agents.includes(agent.id)))
    .filter(connection => `${connectionName(host, connection)} ${connection.id}`.toLowerCase().includes(query.trim().toLowerCase()));
  return <div className="flex max-w-4xl flex-col gap-8">
    <div className="flex flex-col gap-2"><h2 className="text-xl font-medium">Configure</h2><p className="text-muted-foreground">The creation preset and capabilities this agent uses.</p></div>
    <section className="profile-settings-section" aria-labelledby="profile-foundation"><div><h3 id="profile-foundation" className="font-medium">Identity</h3><p className="mt-1 text-muted-foreground">Identity and core setup</p></div><div className="flex min-w-0 flex-col gap-5">
      <dl className="grid grid-cols-2 gap-5"><div><dt className="text-xs text-muted-foreground">Name</dt><dd className="mt-1 break-words">{displayName(agent.name)}</dd></div><div><dt className="text-xs text-muted-foreground">Creation preset</dt><dd className="mt-1 break-words">{agent.preset_id ?? "None"}</dd></div></dl>
      <div className="flex flex-col gap-2"><h4 className="text-sm font-medium">Model & instructions</h4><p className="text-sm text-muted-foreground">The Host does not report this agent’s model or instructions. These settings cannot be edited here yet.</p></div>
    </div></section>
    <Separator />
    <section className="profile-settings-section" aria-labelledby="profile-connections"><div><h3 id="profile-connections" tabIndex={-1} className="scroll-mt-20 font-medium">Tools & connections</h3><p className="mt-1 text-muted-foreground">Selected from the shared library</p><Badge variant="secondary" className="mt-3">{selected.length} selected</Badge></div><div className="flex min-w-0 flex-col gap-4">
      <p className="text-sm text-muted-foreground">Selections belong to this agent. Selection editing is not available yet; existing work may use an earlier configuration.</p>
      {host.connections.length > 0 && <FieldGroup><Field><FieldLabel htmlFor={searchId} className="sr-only">Find a connection</FieldLabel><Input id={searchId} type="search" placeholder="Find a connection…" value={query} onChange={event => setQuery(event.target.value)} /></Field></FieldGroup>}
      <div className="flex flex-col">
        {connections.map(connection => {
          const name = connectionName(host, connection);
          const checked = connection.selected_by_agents.includes(agent.id);
          const peers = host.agents.filter(item => item.id !== agent.id && !isEarlier(item) && connection.selected_by_agents.includes(item.id));
          return <div key={connection.id} className="flex items-start gap-3 border-b py-4 first:pt-0 last:border-0">
            <Checkbox className="mt-1" checked={checked} disabled aria-label={`${name}: ${checked ? "selected" : "not selected"}`} />
            <details className="min-w-0 flex-1"><summary className="text-sm font-medium"><span className="break-all">{name}</span><span className="ml-2 text-xs font-normal text-muted-foreground">{connection.catalog_available ? `${connection.tool_count} tools` : "Catalog unavailable"}</span></summary>
              <div className="mt-3 flex flex-col gap-3 text-xs text-muted-foreground"><p>Stored catalog · Connection health has not been checked.</p>
                {peers.length > 0 && <div className="flex flex-wrap items-center gap-2"><span>Also selected by</span>{peers.map(peer => <Badge asChild key={peer.id} variant="outline"><a href={agentHref(peer.id, "connections")}>{displayName(peer.name)}</a></Badge>)}</div>}
                <p>Connection <code>{connection.id}</code></p>
              </div>
            </details>
          </div>;
        })}
        {!connections.length && <Empty><EmptyHeader><EmptyTitle>{query ? "No matching connections" : "No connections installed"}</EmptyTitle><EmptyDescription>{query ? "Try another name or connection ID." : "Host connections will appear here when installed."}</EmptyDescription></EmptyHeader></Empty>}
      </div>
      <Button variant="outline" className="w-fit" asChild><a href="#library">Manage Host connections</a></Button>
    </div></section>
    <Separator />
    <section className="profile-settings-section" aria-labelledby="profile-skills"><div><h3 id="profile-skills" className="font-medium">Skills</h3><p className="mt-1 text-muted-foreground">Reusable methods and instructions</p></div><p className="text-sm text-muted-foreground">The Host lists installed skills, but does not report which ones are assigned to this agent.</p></section>
    <Separator />
    <details className="text-sm"><summary className="font-medium">Identity & ownership</summary><dl className="mt-5 grid gap-5 sm:grid-cols-2"><div><dt className="text-xs text-muted-foreground">Agent ID</dt><dd className="mt-1"><code>{agent.id}</code></dd></div><div><dt className="text-xs text-muted-foreground">Host ID</dt><dd className="mt-1"><code>{host.host_id}</code></dd></div>{creator && <div><dt className="text-xs text-muted-foreground">Created by</dt><dd className="mt-1"><a className="underline underline-offset-4" href={agentHref(creator.id)}>{displayName(creator.name)}</a></dd></div>}</dl></details>
  </div>;
}

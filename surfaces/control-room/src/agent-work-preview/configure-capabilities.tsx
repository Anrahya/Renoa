import { useEffect, useId, useState } from "react";
import { CaretRight, ArrowUpRight } from "@phosphor-icons/react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Empty, EmptyHeader, EmptyTitle, EmptyDescription } from "@/components/ui/empty";
import { Field, FieldGroup, FieldLabel, FieldSet, FieldLegend, FieldDescription } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { capabilityPlugins, pluginCapabilities, setCapabilities, type CapabilityPlugin } from "./configuration-model";

import { usePreviewConnections } from "./connection-state";

type Selection = { selected: string[]; change: (ids: string[]) => void };
export function ConfigureCapabilities({ selected, change }: Selection) {
  const [query, setQuery] = useState("");
  const prefix = useId();
  const term = query.trim().toLowerCase();
  const plugins = capabilityPlugins.map(plugin => ({ plugin, groups: plugin.groups.map(group => ({ ...group, items: group.items.filter(item => `${plugin.name} ${group.kind} ${group.via ?? ""} ${item.name} ${item.detail}`.toLowerCase().includes(term)) })).filter(group => group.items.length) })).filter(({ groups }) => groups.length);
  return <div className="flex min-w-0 flex-col gap-4">
    <FieldGroup><Field><FieldLabel htmlFor={`${prefix}-search`}>Plugins</FieldLabel><FieldDescription>Expand a plugin to select its individual capabilities.</FieldDescription><Input id={`${prefix}-search`} type="search" aria-label="Find a capability" placeholder="Find plugins, tools or skills…" value={query} onChange={event => setQuery(event.target.value)} /></Field></FieldGroup>
    <div className="config-capability-list">{plugins.map(({ plugin, groups }) => <PluginCapabilities key={plugin.id} {...{ plugin, groups, selected, change, prefix }} searching={!!term} />)}</div>
    {!plugins.length && <Empty><EmptyHeader><EmptyTitle>No matching capabilities</EmptyTitle><EmptyDescription>Try a plugin, tool or skill name.</EmptyDescription></EmptyHeader><Button type="button" size="sm" variant="outline" onClick={() => setQuery("")}>Clear search</Button></Empty>}
    <p className="config-note">Selections apply to this agent. Plugins stay in the Host library when unchecked; connections supply account access where needed.</p>
    <Button asChild variant="outline" className="w-fit"><a href="#library/accounts">Manage Host connections<ArrowUpRight data-icon="inline-end" /></a></Button>
  </div>;
}
function PluginCapabilities({ plugin, groups, selected, change, prefix, searching }: Selection & { plugin: CapabilityPlugin; groups: CapabilityPlugin["groups"]; prefix: string; searching: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const { connected } = usePreviewConnections();
  const items = pluginCapabilities(plugin);
  const count = items.filter(item => selected.includes(item.id)).length;
  const checked = count === items.length ? true : count ? "indeterminate" : false;
  useEffect(() => { if (searching) setExpanded(true); }, [searching]);
  return <div className="config-source" data-selected={count > 0}>
    <div className="config-source-heading"><Checkbox checked={checked} aria-label={`Select all ${plugin.name}`} onCheckedChange={value => change(setCapabilities(selected, items.map(item => item.id), value === true))} />
      <button type="button" className="config-source-toggle" aria-expanded={expanded} aria-controls={`${prefix}-plugin-${plugin.id}`} onClick={() => setExpanded(value => !value)}><span><span className="config-source-name">{plugin.name}</span><small>{plugin.detail}</small></span><span className="config-source-count" aria-label={`${count} of ${items.length} capabilities selected`}>{count}/{items.length}</span><CaretRight className="config-chevron" size={15} /></button>
    </div>
    <div id={`${prefix}-plugin-${plugin.id}`} hidden={!expanded} className="config-source-body">
      {groups.map(group => <FieldSet key={`${group.kind}-${group.via}`}><FieldLegend variant="label"><span className="config-capability-kind">{group.kind}{group.via && <Badge variant="outline">{group.via}</Badge>}</span></FieldLegend><FieldGroup>{group.items.map(item => <Field key={item.id} orientation="horizontal" className="config-capability"><Checkbox id={`${prefix}-capability-${item.id}`} checked={selected.includes(item.id)} onCheckedChange={value => change(setCapabilities(selected, [item.id], value === true))} /><div className="flex min-w-0 flex-col gap-1"><FieldLabel htmlFor={`${prefix}-capability-${item.id}`}>{item.name}</FieldLabel><FieldDescription>{item.detail}</FieldDescription></div></Field>)}</FieldGroup></FieldSet>)}
      {plugin.connectionId && !connected[plugin.connectionId] && count > 0 && <p className="config-connection-note">Selected for this agent. Connect the account in the Host library before these tools can run.</p>}
    </div>
  </div>;
}

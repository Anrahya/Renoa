import { useEffect, useId, useState } from "react";
import { CaretRight, ArrowUpRight } from "@phosphor-icons/react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Empty, EmptyHeader, EmptyTitle, EmptyDescription } from "@/components/ui/empty";
import { Field, FieldGroup, FieldLabel, FieldSet, FieldLegend, FieldDescription } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { capabilitySources, setCapabilities } from "./configuration-model";
type Source = typeof capabilitySources[number];
export function ConfigureCapabilities({ selected, change }: { selected: string[]; change: (ids: string[]) => void }) {
  const [query, setQuery] = useState("");
  const prefix = useId();
  const term = query.trim().toLowerCase();
  const sources = capabilitySources.map(source => ({ ...source, items: source.items.filter(item => `${source.name} ${source.kind} ${item.name} ${item.detail}`.toLowerCase().includes(term)) })).filter(source => source.items.length);
  return <div className="flex min-w-0 flex-col gap-4">
    <FieldGroup><Field><FieldLabel htmlFor={`${prefix}-search`} className="sr-only">Find a capability</FieldLabel><Input id={`${prefix}-search`} type="search" placeholder="Find tools, MCPs or skills…" value={query} onChange={event => setQuery(event.target.value)} /></Field></FieldGroup>
    <div className="config-capability-list">{sources.map(source => <CapabilitySource key={source.id} {...{ source, selected, change, prefix }} searching={!!term} />)}</div>
    {!sources.length && <Empty><EmptyHeader><EmptyTitle>No matching capabilities</EmptyTitle><EmptyDescription>Try a tool, source or skill name.</EmptyDescription></EmptyHeader><Button type="button" size="sm" variant="outline" onClick={() => setQuery("")}>Clear search</Button></Empty>}
    <p className="config-note">Turning a capability off keeps it in the library. Connections supply access; tools that use public endpoints do not need one.</p>
    <Button asChild variant="outline" className="w-fit"><a href="#library">Manage Host connections<ArrowUpRight data-icon="inline-end" /></a></Button>
  </div>;
}
function CapabilitySource({ source, selected, change, prefix, searching }: { source: Source; selected: string[]; change: (ids: string[]) => void; prefix: string; searching: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const all = capabilitySources.find(item => item.id === source.id)!;
  const count = all.items.filter(item => selected.includes(item.id)).length;
  const checked = count === all.items.length ? true : count ? "indeterminate" : false;
  useEffect(() => { if (searching) setExpanded(true); }, [searching]);
  const open = expanded;
  return <div className="config-source" data-selected={count > 0}>
    <div className="config-source-heading"><Checkbox checked={checked} aria-label={`Select all ${source.name}`} onCheckedChange={value => change(setCapabilities(selected, all.items.map(item => item.id), value === true))} />
      <button type="button" className="config-source-toggle" aria-expanded={open} aria-controls={`${prefix}-${source.id}`} onClick={() => setExpanded(value => !value)}><span><span className="config-source-name">{source.name}<Badge variant="outline">{source.kind}</Badge></span><small>{source.detail}</small></span><span className="config-source-count">{count}/{all.items.length}</span><CaretRight className="config-chevron" size={15} /></button>
    </div>
    <div id={`${prefix}-${source.id}`} hidden={!open} className="config-source-body"><FieldSet><FieldLegend className="sr-only">{source.name} capabilities</FieldLegend><FieldGroup>{source.items.map(item => <Field key={item.id} orientation="horizontal" className="config-capability"><Checkbox id={`${prefix}-${item.id}`} checked={selected.includes(item.id)} onCheckedChange={value => change(setCapabilities(selected, [item.id], value === true))} /><div className="flex min-w-0 flex-col gap-1"><FieldLabel htmlFor={`${prefix}-${item.id}`}>{item.name}</FieldLabel><FieldDescription>{item.detail}</FieldDescription></div></Field>)}</FieldGroup></FieldSet>
      {source.needsConnection && count > 0 && <p className="config-connection-note">Selected for this agent. Connect Personal Drive in the Host library before these tools can run.</p>}
    </div>
  </div>;
}

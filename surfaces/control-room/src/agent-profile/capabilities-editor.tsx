import { useState } from "react";
import { capabilities, type CapabilityGroup } from "./model";
import type { SetupProps } from "./setup-editor";

const groups: { id: CapabilityGroup; title: string; description: string }[] = [
  { id: "tool", title: "Tools", description: "Actions it can take" },
  { id: "skill", title: "Skills", description: "Reusable ways of working" },
  { id: "connection", title: "MCP & connections", description: "Services from your Host library" },
];
export function CapabilitiesEditor({ draft, onChange }: SetupProps) {
  const [query, setQuery] = useState("");
  const [expanded, setExpanded] = useState<CapabilityGroup[]>(["tool", "connection"]);
  const filtered = capabilities.filter(item => `${item.name} ${item.detail}`.toLowerCase().includes(query.trim().toLowerCase()));
  return <>
    <div className="pe-section-heading"><h3>Capabilities</h3><p>Check what this agent can use. Unchecked items stay here.</p></div>
    <label className="pe-field pe-search"><span className="sr-only">Search capabilities</span><input type="search" placeholder="Find a tool, skill, or connection" value={query} onChange={e => setQuery(e.target.value)} /></label>
    <div className="pe-capability-groups">{groups.map(group => {
      const items = filtered.filter(item => item.group === group.id);
      const total = capabilities.filter(item => item.group === group.id);
      const selected = total.filter(item => draft.capabilities.includes(item.id)).length;
      if (query && !items.length) return null;
      return <details key={group.id} open={Boolean(query) || expanded.includes(group.id)} className="pe-capability-group">
        <summary onClick={event => { event.preventDefault(); setExpanded(current => current.includes(group.id) ? current.filter(id => id !== group.id) : [...current, group.id]); }}>
          <span>{group.title}<small>{group.description}</small></span><span className="pe-count">{selected} / {total.length}</span>
        </summary>
        <div>{items.map(item => <label key={item.id} className="pe-capability-option">
          <input type="checkbox" checked={draft.capabilities.includes(item.id)} onChange={event => onChange({ ...draft, capabilities: event.target.checked ? [...draft.capabilities, item.id] : draft.capabilities.filter(id => id !== item.id) })} />
          <span><strong>{item.name}</strong><small>{item.detail}</small>{!item.available && draft.capabilities.includes(item.id) && <small className="pe-warning">Selected, but unavailable until reconnected in the Host.</small>}</span>
        </label>)}</div>
      </details>;
    })}</div>
    {!filtered.length && <div className="pe-empty"><p>No capabilities match “{query}”.</p><button type="button" className="ap-text-button" onClick={() => setQuery("")}>Clear search</button></div>}
    <p className="pe-note">Selections apply to this agent. Shared tools and connected accounts remain in the Host library.</p>
    {draft.capabilities.includes("bash") && <p className="pe-note">Bash can access files independently of the file-tool selections.</p>}
  </>;
}

import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import type { Controls } from "./host-controls";
import { agentHref } from "./host-presentation";
import { parseCreationOptions, pendingOperation, readOwner, validCreatedAgent, writeOwner, type CreationOptions } from "./agent-onboarding-client";

const path = "/v1/host/agents";
export function CreateAgent({ controls }: { controls: Controls }) {
  const [open, setOpen] = useState(false);
  const [options, setOptions] = useState<CreationOptions | null>(null);
  const [name, setName] = useState("");
  const [instructions, setInstructions] = useState("");
  const [tools, setTools] = useState<string[]>([]);
  const [model, setModel] = useState("");
  const [reasoning, setReasoning] = useState("");
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    if (!open || controls.preview) return;
    const controller = new AbortController();
    try {
      const saved = pendingOperation(controls.hostId, path);
      setPending(saved !== null);
      if (saved) {
        if (typeof saved.name !== "string" || typeof saved.instructions !== "string" || !Array.isArray(saved.tools) || !saved.tools.every(tool => typeof tool === "string")) throw new Error("The saved creation request is malformed. Restore this tab’s storage before retrying.");
        setName(saved.name); setInstructions(saved.instructions); setTools(saved.tools);
        if (saved.model && typeof saved.model === "object") { const preference = saved.model as { provider: string; model: string; reasoning: string | null }; setModel(`${preference.provider}/${preference.model}`); setReasoning(preference.reasoning ?? ""); }
      }
    } catch (error) { setNotice(String(error)); return; }
    readOwner(`${path}/options`, parseCreationOptions, controller.signal).then(value => {
      setOptions(value);
      if (!pendingOperation(controls.hostId, path)) {
        setModel(current => current || `${value.default_model.provider}/${value.default_model.model}`);
        setReasoning(current => current || value.default_model.reasoning || value.models.find(item => item.provider === value.default_model.provider && item.model === value.default_model.model)?.default_reasoning || "");
      }
    }).catch(error => { if (!controller.signal.aborted) setNotice(error.message); });
    return () => controller.abort();
  }, [open, controls.hostId, controls.preview, attempt]);
  const selected = options?.models.find(item => `${item.provider}/${item.model}` === model);
  async function create(event: React.FormEvent) {
    event.preventDefault(); if (busy || controls.preview) return;
    setBusy(true); setNotice(null);
    try {
      const result = await writeOwner(controls.hostId, path, { name, instructions, tools, connections: [], preset_id: null, automation: null, behavior: null, documents: { soul: true, user: true }, model: selected ? { provider: selected.provider, model: selected.model, reasoning: reasoning || null } : null }, validCreatedAgent);
      setPending(result.kind === "uncertain");
      if (result.kind === "saved") { controls.refresh(); window.location.hash = agentHref(String(result.record.id), "configure"); }
      else setNotice(result.message);
    } catch (error) { setNotice(error instanceof Error ? error.message : "Could not preserve the creation request."); }
    finally { setBusy(false); }
  }
  if (!open) return <Button disabled={controls.preview || !controls.available} onClick={() => setOpen(true)}>Create agent</Button>;
  return <section className="w-full rounded-lg border p-5" aria-labelledby="create-agent-title">
    <form onSubmit={event => void create(event)} className="flex max-w-2xl flex-col gap-5">
      <div><h2 id="create-agent-title" className="text-xl font-medium">Create agent</h2><p className="mt-1 text-sm text-muted-foreground">Give it a job and choose its machine access. Every agent can discover and manage plugins.</p></div>
      <label className="flex flex-col gap-2 text-sm">Name<Input required maxLength={512} value={name} disabled={busy || pending} onChange={event => setName(event.target.value)} autoFocus /></label>
      <label className="flex flex-col gap-2 text-sm">Instructions<textarea required rows={5} className="rounded-md border bg-transparent p-3" value={instructions} disabled={busy || pending} onChange={event => setInstructions(event.target.value)} /></label>
      <div className="grid gap-4 sm:grid-cols-2"><label className="flex flex-col gap-2 text-sm">Model<select className="rounded-md border bg-background p-2" value={model} disabled={busy || pending || !options} onChange={event => { setModel(event.target.value); const next = options?.models.find(item => `${item.provider}/${item.model}` === event.target.value); setReasoning(next?.default_reasoning ?? ""); }}><option value="">Choose a model</option>{options?.models.map(item => <option key={`${item.provider}/${item.model}`} value={`${item.provider}/${item.model}`}>{item.name} · {item.provider}</option>)}</select></label>
      <label className="flex flex-col gap-2 text-sm">Reasoning<select className="rounded-md border bg-background p-2" value={reasoning} disabled={busy || pending || !selected} onChange={event => setReasoning(event.target.value)}><option value="">Model default</option>{selected?.reasoning_levels.map(level => <option key={level} value={level}>{level}</option>)}</select></label></div>
      <fieldset disabled={busy || pending || !options}><legend className="mb-3 text-sm font-medium">Machine access</legend><div className="flex flex-wrap gap-4">{options?.native_tools.map(tool => <label key={tool} className="flex items-center gap-2 text-sm"><Checkbox checked={tools.includes(tool)} onCheckedChange={checked => setTools(values => checked === true ? [...values, tool] : values.filter(value => value !== tool))} />{tool}</label>)}</div><p className="mt-3 text-xs text-muted-foreground">No machine tools are selected by default. Plugin management cannot grant this agent Bash later.</p></fieldset>
      {notice && <p role="status" className="text-sm">{notice}</p>}
      {!options && <Button type="button" variant="outline" onClick={() => { setNotice(null); setAttempt(value => value + 1); }}>Retry loading options</Button>}
      <div className="flex flex-wrap gap-3"><Button type="submit" disabled={busy || !controls.available || (!pending && (!options || !selected || !name.trim() || !instructions.trim()))}>{busy ? "Creating…" : pending ? "Retry saved creation" : "Create agent"}</Button><Button type="button" variant="ghost" disabled={busy} onClick={() => setOpen(false)}>Close</Button></div>
      <p className="text-xs text-muted-foreground">A pending request stays in this tab. Retrying uses the same creation identity.</p>
    </form>
  </section>;
}

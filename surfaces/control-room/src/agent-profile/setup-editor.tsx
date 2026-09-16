import { useState } from "react";
import { CaretRight } from "@phosphor-icons/react";
import { models, starterPresets, type ProfileDraft, type ProfilePart } from "./model";

export type SetupProps = { draft: ProfileDraft; onChange: (next: ProfileDraft) => void };

export function SetupEditor({ draft, onChange, onNavigate }: SetupProps & { onNavigate: (part: ProfilePart) => void }) {
  const [preset, setPreset] = useState<keyof typeof starterPresets>("coding");
  const [applied, setApplied] = useState("");
  return <>
    <div className="pe-section-heading"><h3>Agent basics</h3><p>Who it is and what it is built to do.</p></div>
    <div className="pe-two-fields">
      <label className="pe-field">Name<input required maxLength={40} value={draft.name} onChange={e => onChange({ ...draft, name: e.target.value })} /></label>
      <label className="pe-field">Role<input maxLength={60} value={draft.role} onChange={e => onChange({ ...draft, role: e.target.value })} /></label>
    </div>
    <label className="pe-field">Purpose<textarea required rows={3} value={draft.instructions} onChange={e => onChange({ ...draft, instructions: e.target.value })} /></label>
    <div className="pe-setup-links">
      <button type="button" onClick={() => onNavigate("model")}><span>Model & limits<small>{draft.model} · {draft.maxTokens.toLocaleString()} output tokens</small></span><CaretRight size={16} /></button>
      <button type="button" onClick={() => onNavigate("instructions")}><span>Instructions<small>Purpose, behavior, and your preferences</small></span><CaretRight size={16} /></button>
      <button type="button" onClick={() => onNavigate("tools")}><span>Capabilities<small>{draft.capabilities.length} selected · Tools, skills, and MCP</small></span><CaretRight size={16} /></button>
    </div>
    <details className="pe-presets">
      <summary>Start from a preset <span>Example templates</span></summary>
      <label className="pe-field">Preset<select value={preset} onChange={e => { setPreset(e.target.value as keyof typeof starterPresets); setApplied(""); }}><option value="coding">Coding</option><option value="research">Research</option></select></label>
      <p>{starterPresets[preset].description}</p>
      <p className="pe-note">Replaces purpose and capability selections in this draft. Other settings stay as they are.</p>
      <button className="pe-secondary" type="button" onClick={() => {
        const selected = starterPresets[preset];
        onChange({ ...draft, instructions: selected.instructions, capabilities: [...selected.capabilities] });
        setApplied(`${selected.title} preset applied to this draft.`);
      }}>Use {starterPresets[preset].title.toLowerCase()} preset</button>
      <p className="pe-feedback" role="status">{applied}</p>
    </details>
  </>;
}

export function ModelEditor({ draft, onChange }: SetupProps) {
  return <>
    <div className="pe-section-heading"><h3>Model & limits</h3><p>The engine behind this agent.</p></div>
    <label className="pe-field" htmlFor="pe-model">Model<select id="pe-model" value={draft.model} onChange={e => onChange({ ...draft, model: e.target.value })}>{models.map(model => <option key={model}>{model}</option>)}</select></label>
    <div className="pe-two-fields">
      <label className="pe-field" htmlFor="pe-reasoning">Reasoning<select id="pe-reasoning" value={draft.reasoning} onChange={e => onChange({ ...draft, reasoning: e.target.value })}>{["Low", "Medium", "High", "Max"].map(value => <option key={value}>{value}</option>)}</select></label>
      <label className="pe-field" htmlFor="pe-max-tokens">Max output tokens<input id="pe-max-tokens" type="number" min={1} max={131072} step={1} required value={draft.maxTokens || ""} onChange={e => onChange({ ...draft, maxTokens: Number(e.target.value) })} /></label>
    </div>
    <p className="pe-note">Output limit per response, separate from the context window. Model choices and limits are illustrative in this preview.</p>
  </>;
}

export function InstructionsEditor({ draft, onChange }: SetupProps) {
  return <>
    <div className="pe-section-heading"><h3>Instructions</h3><p>The purpose, behavior, and preferences it works from.</p></div>
    <label className="pe-field">Purpose<textarea required rows={4} value={draft.instructions} onChange={e => onChange({ ...draft, instructions: e.target.value })} /></label>
    <details className="pe-instruction-source" open><summary>Behavior <span>SOUL.md</span></summary><label className="pe-field"><span className="sr-only">Behavior instructions</span><textarea rows={4} value={draft.soul} onChange={e => onChange({ ...draft, soul: e.target.value })} /></label></details>
    <details className="pe-instruction-source"><summary>Your preferences <span>USER.md</span></summary><label className="pe-field"><span className="sr-only">User preferences</span><textarea rows={4} value={draft.user} onChange={e => onChange({ ...draft, user: e.target.value })} /></label></details>
  </>;
}

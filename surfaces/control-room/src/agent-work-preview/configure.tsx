import { useId, useState, type FormEvent } from "react";
import { Check, CaretRight } from "@phosphor-icons/react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Separator } from "@/components/ui/separator";
import { ConfigureCapabilities } from "./configure-capabilities";
import { capabilityPlugins, pluginCapabilities, changedSections, exampleModels, type Configuration } from "./configuration-model";
import type { ConfigurationEditor } from "./configuration-state";
import "../styles/agent-configure-preview.css";
export function ConfigurePreview({ saved, draft, setDraft, save, discard }: ConfigurationEditor) {
  const [notice, setNotice] = useState("");
  const [submitted, setSubmitted] = useState(false);
  const id = useId();
  const changes = changedSections(saved, draft);
  const nameError = submitted && !draft.name.trim();
  const purposeError = submitted && !draft.purpose.trim();
  const tokenError = submitted && (!Number.isInteger(draft.maxTokens) || draft.maxTokens < 1 || draft.maxTokens > 131072);
  const needsConnection = capabilityPlugins.some(plugin => plugin.needsConnection && pluginCapabilities(plugin).some(item => draft.capabilities.includes(item.id)));
  function update<K extends keyof Configuration>(key: K, value: Configuration[K]) { setDraft(current => ({ ...current, [key]: value })); setNotice(""); }
  function jump(section: string) { const target = document.getElementById(`${id}-${section}`); target?.scrollIntoView({ block: "start", behavior: "instant" }); target?.focus({ preventScroll: true }); }
  function submit(event: FormEvent) {
    event.preventDefault(); setSubmitted(true);
    if (!draft.name.trim() || !draft.purpose.trim() || !Number.isInteger(draft.maxTokens) || draft.maxTokens < 1 || draft.maxTokens > 131072) {
      const target = !draft.name.trim() ? "name" : !draft.purpose.trim() ? "purpose" : "tokens";
      document.getElementById(`${id}-${target}`)?.focus(); return;
    }
    const next = { ...draft, name: draft.name.trim(), purpose: draft.purpose.trim() };
    save(next); setSubmitted(false); setNotice("Saved in this preview");
  }
  return <form className="config-preview" onSubmit={submit} noValidate>
    <div className="work-heading"><div><h2>Configure</h2><p>The pieces that make this agent yours.</p></div><Badge variant="outline">Example configuration</Badge></div>
    <nav className="config-composition" aria-label="Configuration sections"><button type="button" onClick={() => jump("model")}><i /><span>Model<strong>{draft.model}</strong></span></button><button type="button" onClick={() => jump("instructions")}><i /><span>Instructions<strong>Purpose & behavior</strong></span></button><button type="button" onClick={() => jump("capabilities")}><i /><span>Capabilities<strong>{draft.capabilities.length} selected{needsConnection ? " · access needed" : ""}</strong></span></button></nav>
    <section className="config-section" aria-labelledby={`${id}-model`}><div className="config-section-label"><h3 id={`${id}-model`} tabIndex={-1}>Model & identity</h3><p>Name the agent and choose how it responds.</p></div><FieldGroup>
      <Field data-invalid={nameError}><FieldLabel htmlFor={`${id}-name`}>Agent name</FieldLabel><Input id={`${id}-name`} maxLength={60} value={draft.name} onChange={event => update("name", event.target.value)} aria-invalid={nameError} aria-describedby={nameError ? `${id}-name-error` : undefined} />{nameError && <FieldError id={`${id}-name-error`}>Enter a name for this agent.</FieldError>}</Field>
      <Field><FieldLabel htmlFor={`${id}-model-select`}>Model</FieldLabel><NativeSelect id={`${id}-model-select`} value={draft.model} onChange={event => update("model", event.target.value)} className="w-full">{exampleModels.map(model => <NativeSelectOption key={model}>{model}</NativeSelectOption>)}</NativeSelect><FieldDescription>Example model choices for this design preview.</FieldDescription></Field>
      <FieldGroup className="config-two-fields"><Field><FieldLabel htmlFor={`${id}-reasoning`}>Reasoning effort</FieldLabel><NativeSelect id={`${id}-reasoning`} value={draft.reasoning} onChange={event => update("reasoning", event.target.value)}>{["Low", "Medium", "High"].map(value => <NativeSelectOption key={value}>{value}</NativeSelectOption>)}</NativeSelect><FieldDescription>How much reasoning to request.</FieldDescription></Field><Field data-invalid={tokenError}><FieldLabel htmlFor={`${id}-tokens`}>Max output tokens</FieldLabel><Input id={`${id}-tokens`} type="number" inputMode="numeric" min={1} max={131072} step={1} value={draft.maxTokens || ""} onChange={event => update("maxTokens", Number(event.target.value))} aria-invalid={tokenError} aria-describedby={tokenError ? `${id}-token-error` : undefined} />{tokenError ? <FieldError id={`${id}-token-error`}>Use a whole number from 1 to 131,072.</FieldError> : <FieldDescription>Per response, not the context window.</FieldDescription>}</Field></FieldGroup>
    </FieldGroup></section>
    <Separator />
    <section className="config-section" aria-labelledby={`${id}-instructions`}><div className="config-section-label"><h3 id={`${id}-instructions`} tabIndex={-1}>Instructions</h3><p>What it does, how it behaves, and what it should know about you.</p></div><FieldGroup>
      <Field data-invalid={purposeError}><FieldLabel htmlFor={`${id}-purpose`}>Purpose</FieldLabel><Textarea id={`${id}-purpose`} rows={4} value={draft.purpose} onChange={event => update("purpose", event.target.value)} aria-invalid={purposeError} aria-describedby={purposeError ? `${id}-purpose-error` : undefined} />{purposeError && <FieldError id={`${id}-purpose-error`}>Describe what this agent should do.</FieldError>}</Field>
      <details className="config-instruction"><summary><CaretRight size={15} />Behavior<span>Tone & working style</span></summary><Field><FieldLabel htmlFor={`${id}-behavior`} className="sr-only">Behavior instructions</FieldLabel><Textarea id={`${id}-behavior`} rows={3} value={draft.behavior} onChange={event => update("behavior", event.target.value)} /></Field></details>
      <details className="config-instruction"><summary><CaretRight size={15} />Your preferences<span>Personal context</span></summary><Field><FieldLabel htmlFor={`${id}-preferences`} className="sr-only">Your preferences</FieldLabel><Textarea id={`${id}-preferences`} rows={3} value={draft.preferences} onChange={event => update("preferences", event.target.value)} /></Field></details>
    </FieldGroup></section>
    <Separator />
    <section className="config-section" aria-labelledby={`${id}-capabilities`}><div className="config-section-label"><h3 id={`${id}-capabilities`} tabIndex={-1}>Capabilities</h3><p>Plugins bundle tools, skills and integrations. Choose what this agent can use.</p><span className="config-selection-count">{draft.capabilities.length} capabilities selected</span></div><ConfigureCapabilities selected={draft.capabilities} change={value => update("capabilities", value)} /></section>
    <footer className="config-save-bar" data-dirty={changes.length > 0}><div><p role="status" className="config-save-status">{notice ? <><Check size={15} />{notice}</> : changes.length ? "Unsaved changes" : "Preview only · no live changes"}</p><span className="config-change-summary">{changes.length ? changes.join(" · ") : "Kept while browsing; reload resets examples."}</span></div><div className="config-save-actions"><Button type="button" variant="ghost" disabled={!changes.length} onClick={() => { discard(); setSubmitted(false); setNotice("Draft discarded"); }}>Discard</Button><Button type="submit" disabled={!changes.length}>Save preview</Button></div></footer>
  </form>;
}

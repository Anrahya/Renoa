import { useId, useRef, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Field, FieldContent, FieldDescription, FieldGroup, FieldLabel, FieldLegend, FieldSet } from "@/components/ui/field";
import { Switch } from "@/components/ui/switch";
import { Alert, AlertDescription } from "@/components/ui/alert";
import type { ReviewRepository, ReviewTrigger, Routine } from "./host-contract";
import { triggerLabels, useChange, type Controls } from "./host-controls";
import { scheduleText, timestamp } from "./host-presentation";

export function ProfileRoutine({ routine, controls }: { routine: Routine; controls: Controls }) {
  const change = useChange(controls, `/v1/host/routines/${routine.id}/enabled`);
  const id = useId();
  return <article className="flex flex-col gap-4 border-b pb-5 last:border-0">
    <Field orientation="horizontal" data-disabled={change.disabled || change.pending}>
      <FieldContent><FieldLabel htmlFor={id}>{routine.name}</FieldLabel><FieldDescription>{scheduleText(routine)}</FieldDescription></FieldContent>
      <Switch id={id} aria-label={`Enable schedule: ${routine.name}`} checked={routine.enabled} disabled={change.disabled || change.pending} onCheckedChange={enabled => void change.save({ expected_revision: routine.revision, enabled })} />
    </Field>
    <div className="flex flex-wrap items-center gap-2"><Badge variant="outline">{routine.enabled ? "Enabled" : "Paused"}</Badge><span className="text-xs text-muted-foreground">{routine.enabled ? `Next · ${timestamp(routine.next_due_ms)}` : "Schedule inactive"}</span></div>
    {routine.pending_runs > 0 && <p className="text-sm">{routine.pending_runs} admitted {routine.pending_runs === 1 ? "run" : "runs"}</p>}
    {change.pending && <Button variant="outline" className="w-fit" disabled={change.disabled} onClick={() => void change.save({ expected_revision: routine.revision, enabled: !routine.enabled })}>Retry saved change</Button>}
    {change.notice && <p role="status" className="text-sm text-muted-foreground">{change.notice}</p>}
    {change.busy && <p role="status" className="text-sm text-muted-foreground">Saving…</p>}
    <details className="text-xs text-muted-foreground"><summary>Schedule details</summary><div className="mt-3 flex flex-col gap-2"><p>Revision {routine.revision} · {routine.completed_runs} completed runs</p><p>Pausing stops future scheduling. Work already admitted can still complete.</p><code>{routine.id}</code></div></details>
  </article>;
}

export function ProfileReviewPolicy({ repository, controls }: { repository: ReviewRepository; controls: Controls }) {
  const { policy, revision } = repository;
  const [editing, setEditing] = useState(false);
  const [enabled, setEnabled] = useState(policy.enabled);
  const [drafts, setDrafts] = useState(policy.skip_drafts);
  const [triggers, setTriggers] = useState(policy.triggers);
  const [expected, setExpected] = useState(revision);
  const change = useChange(controls, `/v1/host/repositories/${policy.repository_id}/policy`);
  const editButton = useRef<HTMLButtonElement>(null);
  const id = useId();
  function edit() { setEnabled(policy.enabled); setDrafts(policy.skip_drafts); setTriggers(policy.triggers); setExpected(revision); setEditing(true); }
  function close() { setEditing(false); requestAnimationFrame(() => editButton.current?.focus()); }
  const fields = { expected_revision: expected, enabled, skip_drafts: drafts, triggers };
  const dirty = enabled !== policy.enabled || drafts !== policy.skip_drafts || triggers.length !== policy.triggers.length || triggers.some(trigger => !policy.triggers.includes(trigger));
  const disabled = change.busy || !controls.available || change.pending;
  return <article className="flex flex-col gap-5 border-b pb-6 last:border-0">
    <div className="flex flex-wrap items-center justify-between gap-2"><h3 className="break-all text-sm font-medium">{policy.full_name}</h3><Badge variant="outline">{policy.enabled ? "Enabled" : "Paused"}</Badge></div>
    {!editing && <><p className="text-sm text-muted-foreground">{policy.skip_drafts ? "Draft PRs are skipped." : "Draft PRs are included."}</p><p className="text-sm text-muted-foreground">{policy.triggers.map(t => triggerLabels[t]).join(" · ") || "No automatic triggers selected."}</p></>}
    {!editing && !change.pending && <Button ref={editButton} variant="outline" className="w-fit" disabled={!controls.available || change.busy} onClick={edit}>Edit review policy</Button>}
    {editing && <form className="flex flex-col gap-5" onSubmit={event => { event.preventDefault(); if (change.disabled || change.pending || expected !== revision || !dirty) return; void change.save(fields).then(result => { if (result === "saved") close(); }); }}>
      <FieldGroup>
        <Field orientation="horizontal" data-disabled={disabled}><FieldContent><FieldLabel htmlFor={`${id}-enabled`}>Automatic reviews</FieldLabel><FieldDescription>Admit reviews for the selected repository events.</FieldDescription></FieldContent><Switch autoFocus id={`${id}-enabled`} disabled={disabled} checked={enabled} onCheckedChange={setEnabled} /></Field>
        <FieldSet disabled={disabled}><FieldLegend variant="label">Review when</FieldLegend><FieldGroup>
          {Object.entries(triggerLabels).map(([value, label]) => <Field key={value} orientation="horizontal" data-disabled={disabled}><Checkbox id={`${id}-${value}`} checked={triggers.includes(value as ReviewTrigger)} disabled={disabled} onCheckedChange={checked => setTriggers(previous => checked === true ? [...previous, value as ReviewTrigger] : previous.filter(item => item !== value))} /><FieldLabel htmlFor={`${id}-${value}`}>{label}</FieldLabel></Field>)}
        </FieldGroup></FieldSet>
        <Field orientation="horizontal" data-disabled={disabled}><Checkbox id={`${id}-drafts`} checked={drafts} disabled={disabled} onCheckedChange={checked => setDrafts(checked === true)} /><FieldLabel htmlFor={`${id}-drafts`}>Skip draft pull requests</FieldLabel></Field>
      </FieldGroup>
      {expected !== revision && <Alert variant="destructive"><AlertDescription>This policy changed while you were editing. Discard this draft and reopen the editor to use the latest settings.</AlertDescription></Alert>}
      <p className="text-xs text-muted-foreground">Changes apply to new reviews. A running review keeps its captured policy.</p>
      {dirty || change.pending ? <div className="sticky bottom-3 flex flex-wrap items-center gap-2 rounded-lg border bg-background p-3 shadow-sm"><span className="mr-auto text-xs text-muted-foreground" role="status">{dirty ? "Unsaved changes" : "No changes"}</span><Button disabled={change.disabled || change.pending || expected !== revision || !dirty}>{change.busy ? "Saving…" : "Save policy"}</Button><Button type="button" variant="ghost" disabled={change.busy} onClick={close}>Discard draft</Button></div> : <Button type="button" variant="ghost" className="w-fit" onClick={close}>Done</Button>}
      <p className="text-xs text-muted-foreground">Your draft stays when switching tabs. Leaving this agent or reloading clears an unsaved draft.</p>
    </form>}
    {change.pending && <Button variant="outline" disabled={change.disabled} onClick={() => void change.save(fields)}>Retry saved policy change</Button>}
    {change.notice && <p role="status" className="text-sm text-muted-foreground">{change.notice}</p>}
  </article>;
}

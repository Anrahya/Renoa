import { useId } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field, FieldContent, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Switch } from "@/components/ui/switch";
import type { Automation } from "./host-contract";
import { useChange, type Controls } from "./host-controls";
import { scheduleText, timestamp } from "./host-presentation";

export function ProfileAutomation({ automation, controls }: { automation: Automation; controls: Controls }) {
  const change = useChange(controls, `/v1/host/automations/${automation.id}/enabled`);
  const id = useId();
  return <article className="flex flex-col gap-4 border-b pb-5 last:border-0">
    <Field orientation="horizontal" data-disabled={change.disabled || change.pending}>
      <FieldContent><FieldLabel htmlFor={id}>{automation.name}</FieldLabel><FieldDescription>{scheduleText(automation)}</FieldDescription></FieldContent>
      <Switch id={id} aria-label={`Enable schedule: ${automation.name}`} checked={automation.enabled} disabled={change.disabled || change.pending} onCheckedChange={enabled => void change.save({ expected_revision: automation.revision, enabled })} />
    </Field>
    <div className="flex flex-wrap items-center gap-2"><Badge variant="outline">{automation.enabled ? "Enabled" : "Paused"}</Badge><span className="text-xs text-muted-foreground">{automation.enabled ? `Next · ${timestamp(automation.next_due_ms)}` : "Schedule inactive"}</span></div>
    {automation.pending_runs > 0 && <p className="text-sm">{automation.pending_runs} admitted {automation.pending_runs === 1 ? "run" : "runs"}</p>}
    {change.pending && <Button variant="outline" className="w-fit" disabled={change.disabled} onClick={() => void change.save({ expected_revision: automation.revision, enabled: !automation.enabled })}>Retry saved change</Button>}
    {change.notice && <p role="status" className="text-sm text-muted-foreground">{change.notice}</p>}
    {change.busy && <p role="status" className="text-sm text-muted-foreground">Saving…</p>}
    <details className="text-xs text-muted-foreground"><summary>Schedule details</summary><div className="mt-3 flex flex-col gap-2"><p>Revision {automation.revision} · {automation.completed_runs} completed runs</p><p>Pausing stops future scheduling. Work already admitted can still complete.</p><code>{automation.id}</code></div></details>
  </article>;
}

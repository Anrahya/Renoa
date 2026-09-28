import { useId } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Field, FieldContent, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Switch } from "@/components/ui/switch";
import type { Routine } from "./host-contract";
import { useChange, type Controls } from "./host-controls";
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

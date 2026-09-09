import { useRef, useState } from "react";
import { pendingChange, saveChange } from "./host-mutations";
import type { ReviewRepository, ReviewTrigger, Routine } from "./host-contract";

export interface Controls { hostId: string; refresh: () => void; available: boolean; preview: boolean }

function useChange(controls: Controls, path: string) {
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  let pending = false;
  let storageError: string | null = null;
  try { pending = !controls.preview && pendingChange(controls.hostId, path) !== null; }
  catch { storageError = "Browser storage is unavailable. Restore it before editing Host settings."; }
  async function save(fields: Record<string, unknown>) {
    if (busy || controls.preview || storageError) return;
    setBusy(true); setNotice(null);
    try {
      const result = await saveChange(controls.hostId, path, fields);
      setNotice(result.message);
      if (result.kind !== "uncertain") controls.refresh();
    } catch (error) { setNotice(error instanceof Error ? error.message : "Could not preserve this change in browser storage."); }
    finally { setBusy(false); }
  }
  return { busy, pending, notice: storageError ?? notice,
    disabled: busy || !controls.available || controls.preview || !!storageError, save };
}

export function RoutineControl({ routine, controls }: { routine: Routine; controls: Controls }) {
  const change = useChange(controls, `/v1/host/routines/${routine.id}/enabled`);
  return <div className="host-control">
    <button className="host-action" disabled={change.disabled} onClick={() => void change.save({ expected_revision: routine.revision, enabled: !routine.enabled })}>
      {change.busy ? "Saving…" : change.pending ? "Retry saved change" : routine.enabled ? "Pause schedule" : "Resume schedule"}</button>
    {controls.preview && <span className="host-caption">Preview · controls available on your Host</span>}
    {change.notice && <p role="status" className="host-caption">{change.notice}</p>}
  </div>;
}

export const triggerLabels: Record<ReviewTrigger, string> = {
  opened: "New pull request", reopened: "Reopened pull request", ready_for_review: "Marked ready for review", synchronize: "New commits pushed",
};
export function ReviewPolicy({ repository, controls }: { repository: ReviewRepository; controls: Controls }) {
  const { policy, revision } = repository;
  const [editing, setEditing] = useState(false);
  const [enabled, setEnabled] = useState(policy.enabled);
  const [drafts, setDrafts] = useState(policy.skip_drafts);
  const [triggers, setTriggers] = useState(policy.triggers);
  const [expected, setExpected] = useState(revision);
  const change = useChange(controls, `/v1/host/repositories/${policy.repository_id}/policy`);
  const editButton = useRef<HTMLButtonElement>(null);
  function edit() { setEnabled(policy.enabled); setDrafts(policy.skip_drafts); setTriggers(policy.triggers); setExpected(revision); setEditing(true); }
  function close() { setEditing(false); requestAnimationFrame(() => editButton.current?.focus()); }
  const fields = { expected_revision: expected, enabled, skip_drafts: drafts, triggers };
  return <article className="host-policy">
    <div className="host-row-heading"><h3>{policy.full_name}</h3><span className={`host-state ${policy.enabled ? "" : "host-secondary"}`}>{policy.enabled ? "Enabled" : "Paused"}</span></div>
    <p>{policy.skip_drafts ? "Draft PRs are skipped." : "Draft PRs are included."}</p>
    <p className="host-caption">{policy.triggers.map(t => triggerLabels[t]).join(" · ") || "No automatic triggers selected."}</p>
    {!editing && !change.pending && <button ref={editButton} className="host-action" disabled={!controls.available || change.busy} onClick={edit}>Edit review policy</button>}
    {editing && <form className="host-policy-form" onSubmit={event => { event.preventDefault(); void change.save(fields).then(close); }}>
      <fieldset disabled={change.busy || !controls.available || change.pending}>
        <legend>When should this agent review?</legend>
        <label><input type="checkbox" autoFocus checked={enabled} onChange={e => setEnabled(e.target.checked)} /> Enable automatic reviews</label>
        {Object.entries(triggerLabels).map(([value, label]) => <label key={value}><input type="checkbox" checked={triggers.includes(value as ReviewTrigger)} onChange={e => setTriggers(t => e.target.checked ? [...t, value as ReviewTrigger] : t.filter(x => x !== value))} />{label}</label>)}
        <label><input type="checkbox" checked={drafts} onChange={e => setDrafts(e.target.checked)} /> Skip draft pull requests</label>
      </fieldset>
      {expected !== revision && <p role="status" className="host-error">This policy changed while you were editing. Cancel and reopen to use the latest settings.</p>}
      <p className="host-caption">Changes affect new admissions. A running review keeps its captured policy, and publication checks the current revision before posting.</p>
      <div className="host-actions"><button className="host-primary" disabled={change.disabled || change.pending || expected !== revision}>Save policy</button>
        <button type="button" className="host-link" disabled={change.busy} onClick={close}>Cancel</button></div>
    </form>}
    {change.pending && <button className="host-action" disabled={change.disabled} onClick={() => void change.save(fields)}>Retry saved policy change</button>}
    {change.notice && <p role="status" className="host-caption">{change.notice}</p>}
    {controls.preview && <p className="host-caption">Preview · controls available on your Host</p>}
  </article>;
}

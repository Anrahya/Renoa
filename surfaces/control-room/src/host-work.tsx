import { useState } from "react";
import type { HostSnapshot, Review, Routine, Session } from "./host-contract";
import { ReviewEvidence } from "./host-review";
import { RoutineControl, type Controls } from "./host-controls";

export function timestamp(value: number): string {
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(value);
}
export function scheduleText(routine: Routine): string {
  const s = routine.schedule;
  if (s.kind === "interval") return `Every ${s.hours} ${s.hours === 1 ? "hour" : "hours"}`;
  if (s.kind === "once") return `Once · ${timestamp(Date.parse(s.at))}`;
  return `Daily · ${String(s.hour).padStart(2, "0")}:${String(s.minute).padStart(2, "0")} · ${s.timezone}`;
}
export function agentName(host: HostSnapshot, id: string | null): string {
  const agent = host.agents.find(a => a.id === id);
  return agent ? displayName(agent.name) : "Unidentified agent";
}
export function displayName(name: string): string {
  return name.startsWith("renoa.") ? "Earlier agent record" : name;
}
export function RoutineRow({ routine, controls }: { routine: Routine; controls: Controls }) {
  return <article className="host-routine">
    <div className="host-row-heading"><h3>{routine.name}</h3><span className="host-state">{routine.enabled ? "Scheduled" : "Paused"}</span></div><p>{scheduleText(routine)}</p>
    <p className="host-caption">{routine.enabled ? `Next scheduled occurrence · ${timestamp(routine.next_due_ms)}` : "Schedule inactive"}
      {routine.pending_runs > 0 && ` · ${routine.pending_runs} admitted ${routine.pending_runs === 1 ? "run" : "runs"}`}</p>
    <RoutineControl {...{ routine, controls }} />
    <details className="host-details"><summary>Schedule details</summary><div>
      <p>Revision {routine.revision} · {routine.completed_runs} completed runs</p>
      <p>Scheduling belongs to this Host. An inactive schedule does not cancel work already admitted.</p>
      <code>{routine.id}</code>
    </div></details>
  </article>;
}
export function SessionRow({ session, host, openAgent }: { session: Session; host: HostSnapshot; openAgent: (id: string) => void }) {
  const operation = session.observation === "available" ? session.active_operation ?? session.latest_operation : null;
  const state = session.observation === "unavailable" ? "Records unavailable" : operation?.state.replaceAll("_", " ") ?? "No recorded operation";
  return <details className="host-record">
    <summary><span>{agentName(host, session.agent_id)}</span><span className="host-record-state">{state}</span></summary>
    <div className="host-record-body">
      {session.observation === "unavailable" ? <p className="host-error">{session.reason}</p> : <>
        <p>{session.event_count} recorded events · {session.queued_operations} queued operations</p>
        {operation && ["unfinished", "waiting", "outcome_unknown"].includes(operation.state) &&
          <p>Recorded state only. This does not confirm that a worker is currently running.</p>}
        {operation && <p className="host-caption">Operation <code>{operation.id}</code></p>}
      </>}
      <p className="host-caption">Session <code>{session.id}</code></p>
      {session.agent_id && <button className="host-link" onClick={() => openAgent(session.agent_id!)}>View agent</button>}
    </div>
  </details>;
}
export function ReviewRow({ review, host, openAgent, preview = false }: { review: Review; host: HostSnapshot; openAgent: (id: string) => void; preview?: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const status = review.worker_error && ["not_recorded", "sending"].includes(review.publication) ? "Worker needs attention" :
    { queued: "Queued", prepared: "Prepared", reviewed: "Review complete", skipped: "Skipped", incomplete: "Incomplete", superseded: "Superseded" }[review.state];
  const delivery = { not_recorded: "No publication recorded", sending: "Publication unconfirmed", published: "Published to GitHub", suppressed: "Publication suppressed", needs_attention: "Publication needs attention" }[review.publication];
  const url = `https://github.com/${review.repository.split("/").map(encodeURIComponent).join("/")}/pull/${review.pull_number}`;
  return <details className="host-record" onToggle={event => setExpanded(event.currentTarget.open)}>
    <summary><span>{review.repository} <span className="host-secondary">#{review.pull_number}</span><small className="host-record-meta">{agentName(host, review.agent_id)} · {review.reported_head_sha.slice(0, 7)}</small></span>
      <span className={`host-record-state ${needsAttention(review) ? "host-error" : ""}`}>{status}<small>{delivery}</small></span></summary>
    <div className="host-record-body">
      <p><button className="host-link" onClick={() => openAgent(review.agent_id)}>{agentName(host, review.agent_id)}</button>
        <span className="host-secondary"> · Admitted {timestamp(review.admitted_at_ms)}</span></p>
      <p className="host-caption">Requested commit <code>{review.reported_head_sha}</code></p>
      {review.reviewed_head_sha && <p className="host-caption">Reviewed commit <code>{review.reviewed_head_sha}</code></p>}
      <p>{review.state === "reviewed" ? delivery + ". A completed review is not an approval or a test result."
        : review.state === "incomplete" ? "This review did not complete. Its diagnostics stay here in your Host."
        : "This is the persisted review state; worker liveness is not confirmed."}</p>
      {expanded && (preview ? <p className="host-caption">Example run. Evidence is loaded from your Host when viewing a real review.</p>
        : <ReviewEvidence request={review.request_id} state={`${review.state}:${review.publication}:${review.retry_after_ms}:${review.worker_error}`} />)}
      <a className="host-link" href={url} target="_blank" rel="noreferrer">Open pull request</a>
    </div>
  </details>;
}
export function needsAttention(review: Review): boolean {
  return review.state === "incomplete" || review.publication === "needs_attention" ||
    review.worker_error && ["not_recorded", "sending"].includes(review.publication);
}
export function currentReviews(reviews: Review[]): Review[] {
  // Catalog order is admission sequence. Earlier failures stay in history when
  // another request has been admitted for the same PR; no guessed resolution.
  const seen = new Set<string>();
  return [...reviews].reverse().filter(r => {
    const key = `${r.repository}:${r.pull_number}`;
    if (seen.has(key)) return false;
    seen.add(key); return true;
  });
}
export function WorkView({ host, openAgent, controls }: { host: HostSnapshot; openAgent: (id: string) => void; controls: Controls }) {
  const latest = currentReviews(host.reviews);
  const incomplete = [...host.reviews].reverse().filter(r => r.publication === "needs_attention" ||
    r.worker_error && ["not_recorded", "sending"].includes(r.publication) || latest.includes(r) && r.state === "incomplete");
  const pending = host.sessions.filter(s => s.observation === "unavailable" || s.active_operation || s.queued_operations > 0);
  const pendingReviews = host.reviews.filter(r => ["queued", "prepared"].includes(r.state) && !incomplete.includes(r));
  const reviews = [...host.reviews].reverse();
  const next = host.routines.filter(r => r.enabled).sort((a, b) => a.next_due_ms - b.next_due_ms);
  return <main className="host-content">
    <p className="host-kicker">One Host. All your work.</p><h1>Your system,<br /><em>in motion.</em></h1>
    <p className="host-intro">Follow the work. Shape what happens next.</p>
    <div className="host-overview-line"><span>{host.agents.length} agents</span><span>{next.length} active schedules</span><span>{host.connections.length} shared connections</span></div>
    <section className="host-section"><div className="host-section-heading"><p className="host-eyebrow">Attention</p><h2>{incomplete.length ? "Worth a look." : "Nothing flagged in the latest reviews."}</h2></div>
      {incomplete.length ? incomplete.map(review => <ReviewRow key={review.request_id} {...{ review, host, openAgent }} preview={controls.preview} />)
        : <p className="host-secondary">Earlier attempts remain in review history below.</p>}</section>
    <section className="host-section"><div className="host-section-heading"><p className="host-eyebrow">Now</p><h2>{incomplete.length ? "Other unfinished work" : "Unfinished work"} <small>{pending.length + pendingReviews.length}</small></h2></div>
      {pendingReviews.map(review => <ReviewRow key={review.request_id} {...{ review, host, openAgent }} preview={controls.preview} />)}
      {pending.length ? pending.map(session => <SessionRow key={session.id} {...{ session, host, openAgent }} />)
        : <p className="host-secondary">No unfinished session operations are recorded.</p>}</section>
    <section className="host-section"><div className="host-section-heading"><p className="host-eyebrow">Next</p><h2>On the horizon.</h2></div>
      {next.length ? next.map(routine => <div key={routine.id}><button className="host-link host-owner-link" onClick={() => openAgent(routine.agent_id)}>
        {agentName(host, routine.agent_id)}</button><RoutineRow routine={routine} controls={controls} /></div>)
        : <p className="host-secondary">No active schedules. Agents can still receive work through their surfaces.</p>}</section>
    {!!reviews.length && <details className="host-history host-section"><summary>Review history <span>{reviews.length} recorded attempts</span></summary>
      <p className="host-caption">Includes earlier failures and superseded attempts, newest first.</p>
      {reviews.map(review => <ReviewRow key={review.request_id} {...{ review, host, openAgent }} preview={controls.preview} />)}</details>}
  </main>;
}

import { useState } from "react";
import type { HostSnapshot, Review, Routine, Session } from "./host-contract";
import { ReviewEvidence } from "./host-review";

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
export function RoutineRow({ routine }: { routine: Routine }) {
  return <article className="host-routine">
    <h3>{routine.name}</h3><p>{scheduleText(routine)}</p>
    <p className="host-caption">{routine.enabled ? `Next scheduled occurrence · ${timestamp(routine.next_due_ms)}` : "Schedule inactive"}
      {routine.pending_runs > 0 && ` · ${routine.pending_runs} admitted ${routine.pending_runs === 1 ? "run" : "runs"}`}</p>
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
export function ReviewRow({ review, host, openAgent }: { review: Review; host: HostSnapshot; openAgent: (id: string) => void }) {
  const [expanded, setExpanded] = useState(false);
  const status = { queued: "Queued", prepared: "Prepared", reviewed: "Review complete", skipped: "Skipped", incomplete: "Incomplete", superseded: "Superseded" }[review.state];
  const url = `https://github.com/${review.repository.split("/").map(encodeURIComponent).join("/")}/pull/${review.pull_number}`;
  return <details className="host-record" onToggle={event => setExpanded(event.currentTarget.open)}>
    <summary><span>{review.repository} <span className="host-secondary">#{review.pull_number}</span></span>
      <span className={`host-record-state ${review.state === "incomplete" ? "host-error" : ""}`}>{status}</span></summary>
    <div className="host-record-body">
      <p><button className="host-link" onClick={() => openAgent(review.agent_id)}>{agentName(host, review.agent_id)}</button>
        <span className="host-secondary"> · Admitted {timestamp(review.admitted_at_ms)}</span></p>
      <p className="host-caption">Requested commit <code>{review.reported_head_sha}</code></p>
      {review.reviewed_head_sha && <p className="host-caption">Reviewed commit <code>{review.reviewed_head_sha}</code></p>}
      <p>{review.state === "reviewed" ? "The review completed. Publication status is not included in this record."
        : review.state === "incomplete" ? "This review did not complete. Its diagnostics stay here in your Host."
        : "This is the persisted review state; worker liveness is not confirmed."}</p>
      {expanded && <ReviewEvidence request={review.request_id} state={review.state} />}
      <a className="host-link" href={url} target="_blank" rel="noreferrer">Open pull request</a>
    </div>
  </details>;
}
export function WorkView({ host, openAgent }: { host: HostSnapshot; openAgent: (id: string) => void }) {
  const incomplete = host.reviews.filter(r => r.state === "incomplete");
  const pending = host.sessions.filter(s => s.observation === "unavailable" || s.active_operation || s.queued_operations > 0);
  const reviews = host.reviews.filter(r => r.state !== "incomplete").sort((a, b) => b.admitted_at_ms - a.admitted_at_ms);
  const next = host.routines.filter(r => r.enabled).sort((a, b) => a.next_due_ms - b.next_due_ms);
  return <main className="host-content">
    <p className="host-kicker">Your Host, at a glance</p><h1>The work.</h1>
    <p className="host-intro">What needs your attention, what’s unfinished, and what’s scheduled next.</p>
    <section className="host-section"><h2>Needs attention <small>{incomplete.length}</small></h2>
      {incomplete.length ? incomplete.map(review => <ReviewRow key={review.request_id} {...{ review, host, openAgent }} />)
        : <p className="host-secondary">No incomplete reviews in the recorded history.</p>}</section>
    <section className="host-section"><h2>Unfinished work <small>{pending.length}</small></h2>
      {pending.length ? pending.map(session => <SessionRow key={session.id} {...{ session, host, openAgent }} />)
        : <p className="host-secondary">No unfinished session operations are recorded.</p>}</section>
    <section className="host-section"><h2>Coming up</h2>
      {next.length ? next.map(routine => <div key={routine.id}><button className="host-link host-owner-link" onClick={() => openAgent(routine.agent_id)}>
        {agentName(host, routine.agent_id)}</button><RoutineRow routine={routine} /></div>)
        : <p className="host-secondary">No active schedules. Agents can still receive work through their surfaces.</p>}</section>
    {!!reviews.length && <section className="host-section"><h2>Review history</h2>
      {reviews.map(review => <ReviewRow key={review.request_id} {...{ review, host, openAgent }} />)}</section>}
  </main>;
}

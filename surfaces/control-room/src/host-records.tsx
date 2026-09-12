import { useState } from "react";
import { ArrowUpRight } from "@phosphor-icons/react";
import type { HostSnapshot, Review, Routine, Session } from "./host-contract";
import { ReviewEvidence } from "./host-review";
import { RoutineControl, type Controls } from "./host-controls";
import { agentHref, agentName, needsAttention, scheduleText, sessionNeedsAttention, timestamp } from "./host-presentation";

export function RoutineRow({ routine, controls }: { routine: Routine; controls: Controls }) {
  return <article className="host-routine">
    <div className="host-row-heading"><h3>{routine.name}</h3><span className={`host-state ${routine.enabled ? "" : "host-state-muted"}`}>{routine.enabled ? "Scheduled" : "Paused"}</span></div>
    <p>{scheduleText(routine)}</p>
    <p className="host-caption">{routine.enabled ? `Next scheduled occurrence · ${timestamp(routine.next_due_ms)}` : "Schedule inactive"}
      {routine.pending_runs > 0 && ` · ${routine.pending_runs} admitted ${routine.pending_runs === 1 ? "run" : "runs"}`}</p>
    <RoutineControl {...{ routine, controls }} />
    <details className="host-details"><summary>Schedule details</summary><div>
      <p>Revision {routine.revision} · {routine.completed_runs} completed runs</p>
      <p>Scheduling belongs to this Host. Pausing does not cancel work already admitted.</p><code>{routine.id}</code>
    </div></details>
  </article>;
}
export function SessionRow({ session, host }: { session: Session; host: HostSnapshot }) {
  const operation = session.observation === "available" ? session.active_operation ?? session.latest_operation : null;
  const state = session.observation === "unavailable" ? "Records unavailable" : operation?.state.replaceAll("_", " ") ?? "No recorded operation";
  return <details className="host-record">
    <summary><span>{agentName(host, session.agent_id)}<small className="host-record-meta">Session {session.id.slice(0, 8)}{session.observation === "available" && ` · ${session.event_count} events`}</small></span>
      <span className={`host-record-state ${sessionNeedsAttention(session) ? "host-error" : ""}`}>{state}</span></summary>
    <div className="host-record-body">
      {session.observation === "unavailable" ? <p className="host-error">{session.reason}</p> : <>
        <p>{session.event_count} recorded events · {session.queued_operations} queued operations</p>
        <p className="host-caption">This view contains operation records, not the conversation transcript. An unfinished record does not confirm a running worker.</p>
        {operation && <p className="host-caption">Operation <code>{operation.id}</code></p>}
      </>}
      <p className="host-caption">Session <code>{session.id}</code></p>
      {session.agent_id && <a className="host-link" href={agentHref(session.agent_id)}>View agent</a>}
    </div>
  </details>;
}
export function ReviewRow({ review, host, preview = false }: { review: Review; host: HostSnapshot; preview?: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const status = review.worker_error && ["not_recorded", "sending"].includes(review.publication) ? "Worker needs attention" :
    { queued: "Queued", prepared: "Prepared", reviewed: "Review complete", skipped: "Skipped", incomplete: "Incomplete", superseded: "Superseded" }[review.state];
  const delivery = { not_recorded: "No publication recorded", sending: "Publication unconfirmed", published: "Published to GitHub", suppressed: "Publication suppressed", needs_attention: "Publication needs attention" }[review.publication];
  const url = `https://github.com/${review.repository.split("/").map(encodeURIComponent).join("/")}/pull/${review.pull_number}`;
  return <details className="host-record" onToggle={event => setExpanded(event.currentTarget.open)}>
    <summary><span>{review.repository} <span className="host-secondary">#{review.pull_number}</span>
      <small className="host-record-meta">{agentName(host, review.agent_id)} · {review.reported_head_sha.slice(0, 7)} · {timestamp(review.admitted_at_ms)}</small></span>
      <span className={`host-record-state ${needsAttention(review) ? "host-error" : ""}`}>{status}{" "}<small>{delivery}</small></span></summary>
    <div className="host-record-body">
      <p><a className="host-link" href={agentHref(review.agent_id)}>{agentName(host, review.agent_id)}</a>
        <span className="host-secondary"> · Admitted {timestamp(review.admitted_at_ms)}</span></p>
      <p className="host-caption">Requested commit <code>{review.reported_head_sha}</code></p>
      {review.reviewed_head_sha && <p className="host-caption">Reviewed commit <code>{review.reviewed_head_sha}</code></p>}
      <p>{review.state === "reviewed" ? delivery + ". A completed review is not an approval or a test result."
        : review.state === "incomplete" ? "This review did not complete. Its diagnostics stay here in your Host."
        : "This is the persisted review state; worker liveness is not confirmed."}</p>
      {expanded && (preview ? <p className="host-caption">This preview contains summary records. Full findings and execution evidence are available in the live Host.</p>
        : <ReviewEvidence request={review.request_id} state={`${review.state}:${review.publication}:${review.retry_after_ms}:${review.worker_error}`} />)}
      <a className="host-link" href={url} target="_blank" rel="noreferrer">Open pull request <ArrowUpRight size={15} aria-hidden="true" /></a>
    </div>
  </details>;
}

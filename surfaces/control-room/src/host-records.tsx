import type { HostSnapshot, Session } from "./host-contract";
import { agentHref, agentName, sessionNeedsAttention } from "./host-presentation";

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

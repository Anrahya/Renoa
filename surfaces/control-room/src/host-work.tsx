import type { HostSnapshot } from "./host-contract";
import type { Controls } from "./host-controls";
import { AgentRoster } from "./host-roster";
import { ReviewRow, RoutineRow, SessionRow } from "./host-records";
import { agentHref, agentName, attentionReviews, currentReviews, isEarlier, sessionNeedsAttention, sessionUnfinished } from "./host-presentation";

export function WorkView({ host, controls }: { host: HostSnapshot; controls: Controls }) {
  const attention = attentionReviews(host.reviews);
  const sessions = host.sessions.filter(sessionNeedsAttention);
  const unfinished = host.sessions.filter(s => sessionUnfinished(s) && !sessions.includes(s));
  const pendingReviews = host.reviews.filter(r => ["queued", "prepared"].includes(r.state) && !attention.includes(r));
  const next = host.routines.filter(r => r.enabled).sort((a, b) => a.next_due_ms - b.next_due_ms);
  const recent = currentReviews(host.reviews).filter(r => !attention.includes(r) && !pendingReviews.includes(r));
  const earlier = host.agents.filter(isEarlier).length;
  return <main id="host-main" className="host-content">
    <div className="host-page-heading"><div><h1>Your Host</h1><p className="host-intro">A place for all your agents. Open one to follow its work.</p></div>
      {earlier > 0 && <a href="#agents" className="host-link host-quiet-link">{earlier} earlier identities</a>}</div>
    <AgentRoster host={host} />
    <section className="host-section host-attention"><div className="host-section-heading"><h2>Needs attention</h2><span className="host-count">{attention.length + sessions.length}</span></div>
      {attention.map(review => <ReviewRow key={review.request_id} {...{ review, host }} preview={controls.preview} />)}
      {sessions.map(session => <SessionRow key={session.id} {...{ session, host }} />)}
      {!attention.length && !sessions.length && <p className="host-empty">No attention flags in the latest records.</p>}
    </section>
    {!!(unfinished.length + pendingReviews.length) && <section className="host-section"><div className="host-section-heading"><h2>Unfinished work</h2><span className="host-count">{unfinished.length + pendingReviews.length}</span></div>
      <p className="host-caption">Recorded operations, including earlier identities. This is not a worker heartbeat.</p>
      {pendingReviews.map(review => <ReviewRow key={review.request_id} {...{ review, host }} preview={controls.preview} />)}
      {unfinished.map(session => <SessionRow key={session.id} {...{ session, host }} />)}
    </section>}
    <section className="host-section"><div className="host-section-heading"><h2>Coming up</h2><span className="host-count">{next.length}</span></div>
      {next.length ? next.map(routine => <div key={routine.id}><a className="host-link host-owner-link" href={agentHref(routine.agent_id, "automations")}>{agentName(host, routine.agent_id)}</a>
        <RoutineRow {...{ routine, controls }} /></div>) : <p className="host-empty">No enabled schedules. <a href="#agents" className="host-link">View agent automations</a></p>}
    </section>
    {!!recent.length && <section className="host-section"><div className="host-section-heading"><h2>Latest reviews</h2><span className="host-caption">One latest attempt per pull request</span></div>
      {recent.map(review => <ReviewRow key={review.request_id} {...{ review, host }} preview={controls.preview} />)}
    </section>}
    {!!host.reviews.length && <details className="host-history host-section"><summary>All review attempts <span>{host.reviews.length} records</span></summary>
      <p className="host-caption">Earlier failures remain recorded. A newer attempt does not establish that an earlier failure was resolved.</p>
      {[...host.reviews].reverse().map(review => <ReviewRow key={review.request_id} {...{ review, host }} preview={controls.preview} />)}
    </details>}
  </main>;
}

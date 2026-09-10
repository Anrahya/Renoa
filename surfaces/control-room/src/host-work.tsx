import { useState } from "react";
import { Clock, GitPullRequest, Hourglass, WarningCircle } from "@phosphor-icons/react";
import type { HostSnapshot } from "./host-contract";
import type { Controls } from "./host-controls";
import { AgentMap } from "./host-agent-map";
import { ReviewRow, RoutineRow, SessionRow } from "./host-records";
import { agentHref, agentName, attentionReviews, currentReviews, sessionNeedsAttention, sessionUnfinished } from "./host-presentation";

export function WorkView({ host, controls }: { host: HostSnapshot; controls: Controls }) {
  const attention = attentionReviews(host.reviews);
  const sessions = host.sessions.filter(sessionNeedsAttention);
  const unfinished = host.sessions.filter(s => sessionUnfinished(s) && !sessions.includes(s));
  const pendingReviews = host.reviews.filter(r => ["queued", "prepared"].includes(r.state) && !attention.includes(r));
  const next = host.routines.filter(r => r.enabled).sort((a, b) => a.next_due_ms - b.next_due_ms);
  const [section, setSection] = useState("attention");
  const sections = [
    { id: "attention", name: "Attention", count: attention.length + sessions.length, Icon: WarningCircle },
    { id: "unfinished", name: "Unfinished", count: unfinished.length + pendingReviews.length, Icon: Hourglass },
    { id: "schedules", name: "Schedules", count: next.length, Icon: Clock },
    { id: "reviews", name: "Reviews", count: currentReviews(host.reviews).length, Icon: GitPullRequest },
  ];
  return <main id="host-main" className="host-content">
    <div className="host-page-heading"><div><h1>Your Host</h1><p className="host-intro">Agents, their connections, and recorded work.</p></div>
      <a href="#agents" className="host-link host-quiet-link">Agent directory</a></div>
    <AgentMap host={host} />
    <nav className="host-subnav host-work-tabs" aria-label="Recorded work">{sections.map(({ id, name, count, Icon }) => <button key={id} aria-pressed={section === id} onClick={() => setSection(id)}><Icon size={18} aria-hidden="true" />{name}<span>{count}</span></button>)}</nav>
    {section === "attention" && <section className="host-section host-attention"><h2 className="sr-only">Needs attention</h2>
      {attention.map(review => <ReviewRow key={review.request_id} {...{ review, host }} preview={controls.preview} />)}
      {sessions.map(session => <SessionRow key={session.id} {...{ session, host }} />)}
      {!attention.length && !sessions.length && <p className="host-empty">No attention flags in the latest records.</p>}
    </section>}
    {section === "unfinished" && <section className="host-section"><h2 className="sr-only">Unfinished work</h2>
      <p className="host-caption">Recorded operations, including earlier identities. This is not a worker heartbeat.</p>
      {pendingReviews.map(review => <ReviewRow key={review.request_id} {...{ review, host }} preview={controls.preview} />)}
      {unfinished.map(session => <SessionRow key={session.id} {...{ session, host }} />)}
      {!unfinished.length && !pendingReviews.length && <p className="host-empty">No unfinished work recorded.</p>}
    </section>}
    {section === "schedules" && <section className="host-section"><h2 className="sr-only">Coming up</h2>
      {next.length ? next.map(routine => <div key={routine.id}><a className="host-link host-owner-link" href={agentHref(routine.agent_id, "automations")}>{agentName(host, routine.agent_id)}</a>
        <RoutineRow {...{ routine, controls }} /></div>) : <p className="host-empty">No enabled schedules. <a href="#agents" className="host-link">View agent automations</a></p>}
    </section>}
    {section === "reviews" && <section className="host-section"><h2 className="sr-only">Latest reviews</h2><p className="host-caption">Latest attempt per pull request</p>
      {currentReviews(host.reviews).map(review => <ReviewRow key={review.request_id} {...{ review, host }} preview={controls.preview} />)}
      {!host.reviews.length && <p className="host-empty">No recorded reviews.</p>}
    {!!host.reviews.length && <details className="host-history host-section"><summary>All review attempts <span>{host.reviews.length} records</span></summary>
      <p className="host-caption">Earlier failures remain recorded. A newer attempt does not establish that an earlier failure was resolved.</p>
      {[...host.reviews].reverse().map(review => <ReviewRow key={review.request_id} {...{ review, host }} preview={controls.preview} />)}
    </details>}
    </section>}
  </main>;
}

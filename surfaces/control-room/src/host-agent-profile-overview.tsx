import { ArrowRight } from "@phosphor-icons/react";
import { Button } from "@/components/ui/button";
import type { Agent, HostSnapshot } from "./host-contract";
import type { AgentSection } from "./host-navigation";
import { scheduleText, timestamp } from "./host-presentation";
import { agentOverview } from "./host-agent-overview";
import { ProfileReviewSummary } from "./host-agent-profile-records";
import { WorkDay } from "./host-work-day";
import "./styles/agent-work-preview.css";

export function ProfileOverview({ host, agent, navigate }: { host: HostSnapshot; agent: Agent; navigate: (section: AgentSection) => void }) {
  const data = agentOverview(host, agent);
  const next = data.scheduled[0];
  return <div className="agent-work-preview work-overview">
    <div className="work-heading"><div><h2>At a glance</h2><p>What needs you, what happened, and what starts next.</p></div></div>
    <WorkDay host={host} agents={[agent]} />
    <div className="work-overview-columns"><section><div className="run-section-heading"><h3>Recorded work</h3><Button variant="ghost" size="sm" onClick={() => navigate("activity")}>All activity<ArrowRight data-icon="inline-end" /></Button></div>
      <p className="work-caption">{data.activity.label}</p>
      {data.unavailableSessions > 0 && <p className="work-caption">{data.unavailableSessions} session records unavailable</p>}
      {data.latestReviews.slice(0,3).map(review => <ProfileReviewSummary key={review.request_id} review={review} onOpen={() => navigate("activity")} />)}
      {!data.reviews.length && !data.sessions.length && <p className="work-caption mt-4">No work recorded yet.</p>}
      {data.sessions.length > 0 && <Button variant="outline" className="mt-4" onClick={() => navigate("activity")}>{data.sessions.length} session records</Button>}
    </section><section><div className="run-section-heading"><h3>Coming up</h3><Button variant="ghost" size="sm" onClick={() => navigate("automations")}>Automations<ArrowRight data-icon="inline-end" /></Button></div><div className="work-upcoming">{data.scheduled.slice(0,3).map(routine => <button key={routine.id} onClick={() => navigate("automations")}><time>{new Date(routine.next_due_ms).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })}<small>{new Date(routine.next_due_ms).toLocaleDateString()}</small></time><i /><span><strong>{routine.name}</strong><small>{scheduleText(routine)}</small></span><ArrowRight size={14} /></button>)}{!next && <p className="work-caption">Nothing scheduled</p>}</div><div className="work-overview-automation-note"><span>{data.repositories.filter(repository => repository.policy.enabled).length} repository triggers enabled</span><span>{data.paused.length} schedules paused</span></div>{next && <p className="sr-only">Next scheduled occurrence: {timestamp(next.next_due_ms)}</p>}</section></div>
    <div className="work-overview-configure"><div><h3>How this agent works</h3><p>Model, instructions and capabilities. Connections supply access where a capability needs it.</p></div><Button variant="outline" onClick={() => navigate("configure")}>Configure agent<ArrowRight data-icon="inline-end" /></Button></div>
  </div>;
}

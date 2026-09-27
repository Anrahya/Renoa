import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { Agent, HostSnapshot } from "./host-contract";
import { AgentLink } from "./host-desk";
import { agentHref, needsAttention, timestamp } from "./host-presentation";
import "./styles/host-work-preview.css";

export function WorkDay({ host, agents }: { host: HostSnapshot; agents: Agent[] }) {
  const start = new Date(); start.setHours(0, 0, 0, 0);
  const end = new Date(start); end.setDate(end.getDate() + 1);
  const today = host.reviews.filter(review => review.admitted_at_ms >= start.getTime() && review.admitted_at_ms < end.getTime());
  return <section className="desk-day-map" aria-label="Today across your agents"><div className="desk-section-heading"><h2>Today across your agents</h2><span className="desk-note">{today.filter(review => agents.some(agent => agent.id === review.agent_id)).length} review admissions with recorded times</span></div>
    <div className="desk-day-scroll" tabIndex={0} aria-label="Daily work timeline, scroll horizontally on small screens"><div className="desk-day-lanes"><div className="desk-day-axis"><span /><div>{[0,6,12,18,24].map(hour => <span key={hour} style={{ left: `${hour / 24 * 100}%` }}>{String(hour).padStart(2,"0")}:00</span>)}</div></div>
      {agents.map(agent => <div className="desk-day-lane" key={agent.id}><AgentLink agent={agent} /><div className="desk-day-track">{today.filter(review => review.agent_id === agent.id).map(review => <Tooltip key={review.request_id}><TooltipTrigger asChild><a className="desk-day-mark" data-tone={needsAttention(review) ? "interrupted" : review.state === "reviewed" ? "completed" : "waiting"} style={{ left: `${(review.admitted_at_ms - start.getTime()) / (end.getTime() - start.getTime()) * 100}%`, width: ".9%" }} href={agentHref(agent.id,"activity")} aria-label={`${agent.name}: ${review.repository} #${review.pull_number}, ${timestamp(review.admitted_at_ms)}, ${review.state}`} /></TooltipTrigger><TooltipContent>{review.repository} #{review.pull_number}<br />{timestamp(review.admitted_at_ms)} · {review.state}</TooltipContent></Tooltip>)}</div></div>)}
    </div></div>
    <p className="desk-note mt-4">Marks show admissions, not execution duration. Session summaries have no recorded time in this snapshot.</p>
  </section>;
}

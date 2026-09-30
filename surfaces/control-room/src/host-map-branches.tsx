import { useId, useState, type CSSProperties } from "react";
import { CaretDown, Clock, Hourglass, Pause } from "@phosphor-icons/react";
import type { Automation } from "./host-contract";
import { agentHref, scheduleText } from "./host-presentation";
import { scheduleCountdown, scheduleSummary } from "./host-system-model";

export function AgentSchedules({ automations, now }: { automations: Automation[]; now: number | null }) {
  const [expanded, setExpanded] = useState(false);
  const listId = useId();
  const { ordered, next, pending, paused } = scheduleSummary(automations);
  if (!ordered.length) return null;
  if (ordered.length === 1) return <ul className="system-schedules"><li><ScheduleLink automation={ordered[0]!} now={now} /></li></ul>;
  return <div className="system-schedule-group">
    <button className="system-branch-toggle" aria-expanded={expanded} aria-controls={listId} onClick={() => setExpanded(!expanded)}>
      <Clock size={15} aria-hidden="true" /><span>{ordered.length} schedules</span>
      <span className="system-branch-state">{pending ? `${pending} pending` : paused === ordered.length ? "All paused" : `${ordered.length - paused} enabled`}</span>
      <CaretDown size={14} aria-hidden="true" />
    </button>
    {!expanded && next && <div className="system-next-schedule"><ScheduleLink automation={next} now={now} prefix="Next · " /></div>}
    <div className="system-branch-reveal" data-open={expanded} inert={!expanded}>
      <div><ul id={listId} className="system-schedule-list" aria-label="All agent schedules" tabIndex={expanded ? 0 : -1}>
        {ordered.map(automation => <li key={automation.id}><ScheduleLink automation={automation} now={now} /></li>)}
      </ul></div>
    </div>
  </div>;
}

function ScheduleLink({ automation, now, prefix = "" }: { automation: Automation; now: number | null; prefix?: string }) {
  const value = scheduleCountdown(automation, now);
  const ticking = automation.enabled && !automation.pending_runs && now !== null && automation.next_due_ms > now;
  return <a className="system-schedule-link" href={agentHref(automation.agent_id, "automations")} title={`${automation.name} · ${scheduleText(automation)}`} aria-label={`${prefix}${automation.name}: ${value}`}>
    {ticking ? <span className="system-clock" style={{ "--clock-angle": `${Math.floor(now / 1000) * 6}deg` } as CSSProperties} aria-hidden="true" /> : automation.pending_runs ? <Hourglass size={15} aria-hidden="true" /> : automation.enabled ? <Clock size={15} aria-hidden="true" /> : <Pause size={15} aria-hidden="true" />}
    <span className="system-schedule-name">{prefix}{automation.name}</span><span className="system-timer" aria-hidden="true">{value}</span>
  </a>;
}

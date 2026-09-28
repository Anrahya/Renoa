import { useId, useState, type CSSProperties } from "react";
import { CaretDown, Clock, Hourglass, Pause } from "@phosphor-icons/react";
import type { Routine } from "./host-contract";
import { agentHref, scheduleText } from "./host-presentation";
import { scheduleCountdown, scheduleSummary } from "./host-system-model";

export function AgentSchedules({ routines, now }: { routines: Routine[]; now: number | null }) {
  const [expanded, setExpanded] = useState(false);
  const listId = useId();
  const { ordered, next, pending, paused } = scheduleSummary(routines);
  if (!ordered.length) return null;
  if (ordered.length === 1) return <ul className="system-schedules"><li><ScheduleLink routine={ordered[0]!} now={now} /></li></ul>;
  return <div className="system-schedule-group">
    <button className="system-branch-toggle" aria-expanded={expanded} aria-controls={listId} onClick={() => setExpanded(!expanded)}>
      <Clock size={15} aria-hidden="true" /><span>{ordered.length} schedules</span>
      <span className="system-branch-state">{pending ? `${pending} pending` : paused === ordered.length ? "All paused" : `${ordered.length - paused} enabled`}</span>
      <CaretDown size={14} aria-hidden="true" />
    </button>
    {!expanded && next && <div className="system-next-schedule"><ScheduleLink routine={next} now={now} prefix="Next · " /></div>}
    <div className="system-branch-reveal" data-open={expanded} inert={!expanded}>
      <div><ul id={listId} className="system-schedule-list" aria-label="All agent schedules" tabIndex={expanded ? 0 : -1}>
        {ordered.map(routine => <li key={routine.id}><ScheduleLink routine={routine} now={now} /></li>)}
      </ul></div>
    </div>
  </div>;
}

function ScheduleLink({ routine, now, prefix = "" }: { routine: Routine; now: number | null; prefix?: string }) {
  const value = scheduleCountdown(routine, now);
  const ticking = routine.enabled && !routine.pending_runs && now !== null && routine.next_due_ms > now;
  return <a className="system-schedule-link" href={agentHref(routine.agent_id, "automations")} title={`${routine.name} · ${scheduleText(routine)}`} aria-label={`${prefix}${routine.name}: ${value}`}>
    {ticking ? <span className="system-clock" style={{ "--clock-angle": `${Math.floor(now / 1000) * 6}deg` } as CSSProperties} aria-hidden="true" /> : routine.pending_runs ? <Hourglass size={15} aria-hidden="true" /> : routine.enabled ? <Clock size={15} aria-hidden="true" /> : <Pause size={15} aria-hidden="true" />}
    <span className="system-schedule-name">{prefix}{routine.name}</span><span className="system-timer" aria-hidden="true">{value}</span>
  </a>;
}

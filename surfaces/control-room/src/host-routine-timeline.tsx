import { useEffect, useState } from "react";
import { CaretLeft, CaretRight, Clock } from "@phosphor-icons/react";
import { Button } from "@/components/ui/button";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { HostSnapshot, Routine } from "./host-contract";
import type { Controls } from "./host-controls";
import { ProfileRoutine } from "./host-agent-profile-settings";
import { agentName, scheduleText, timestamp } from "./host-presentation";
import "./styles/agent-work-preview.css";

/** Positions only the next occurrence reported by the Host, in its own timezone. */
export function RoutineTimeline({ routines, host, controls }: { routines: Routine[]; host: HostSnapshot; controls: Controls }) {
  const [range, setRange] = useState("day");
  const [offset, setOffset] = useState(0);
  const [selected, select] = useState<string | null>(routines[0]?.id ?? null);
  const today = new Date(); today.setHours(0, 0, 0, 0);
  const start = new Date(today); start.setDate(start.getDate() + offset * (range === "day" ? 1 : 7));
  const end = new Date(start); end.setDate(end.getDate() + (range === "day" ? 1 : 7));
  const span = end.getTime() - start.getTime();
  const current = routines.find(routine => routine.id === selected);
  useEffect(() => {
    if (!selected) return;
    const frame = requestAnimationFrame(() => document.getElementById("schedule-inspection-title")?.focus({ preventScroll: true }));
    return () => cancelAnimationFrame(frame);
  }, [selected]);
  const date = (at: Date) => at.toLocaleDateString(undefined, { day: "numeric", month: "short" });
  return <div className="agent-work-preview routine-timeline">
    <div className="work-heading"><div><h2>Automations</h2><p>Next occurrences reported by the Host. Recurrence details stay in each schedule.</p></div></div>
    <Tabs value={range} onValueChange={value => { setRange(value); setOffset(0); }} className="gap-5">
      <div className="work-toolbar"><div className="flex items-center gap-2"><Button variant="ghost" size="icon" aria-label="Previous period" onClick={() => setOffset(value => value - 1)}><CaretLeft /></Button><span className="work-period">{date(start)}{range === "week" && ` – ${date(new Date(end.getTime() - 1))}`}</span><Button variant="ghost" size="icon" aria-label="Next period" onClick={() => setOffset(value => value + 1)}><CaretRight /></Button>{offset !== 0 && <Button variant="outline" size="sm" onClick={() => setOffset(0)}>Today</Button>}</div><div className="flex items-center gap-3"><span className="work-caption">Your browser timezone</span><TabsList aria-label="Timeline range"><TabsTrigger value="day">Day</TabsTrigger><TabsTrigger value="week">Week</TabsTrigger></TabsList></div></div>
      <div className="work-timeline-scroll" tabIndex={0} aria-label="Automation timeline, scroll horizontally on small screens"><div className={`work-timeline ${range === "week" ? "work-timeline-week" : ""}`}>
        <div className="work-axis"><span className="work-caption">Automation</span><div className="work-axis-track">{Array.from({ length: range === "day" ? 5 : 7 }, (_, index) => <span key={index} style={{ left: `${(new Date(start.getFullYear(), start.getMonth(), start.getDate() + (range === "week" ? index : 0), range === "day" ? index * 6 : 0).getTime() - start.getTime()) / span * 100}%` }}>{range === "day" ? `${String(index * 6).padStart(2, "0")}:00` : date(new Date(start.getFullYear(), start.getMonth(), start.getDate() + index))}</span>)}</div><span /></div>
        {routines.map(routine => {
          const visible = routine.enabled && routine.next_due_ms >= start.getTime() && routine.next_due_ms < end.getTime();
          return <div className={`work-lane ${selected === routine.id ? "work-lane-selected" : ""}`} key={routine.id}>
            <button className="work-lane-name" aria-expanded={selected === routine.id} aria-controls="schedule-inspection" onClick={() => select(selected === routine.id ? null : routine.id)}><span>{routine.name}<CaretRight size={12} /></span><small>{agentName(host, routine.agent_id)} · {scheduleText(routine)}</small></button>
            <div className="work-track"><div className={`work-track-line ${routine.enabled ? "" : "work-track-paused"}`} />{visible ? <Tooltip><TooltipTrigger asChild><button className="work-mark-hit" style={{ left: `${(routine.next_due_ms - start.getTime()) / span * 100}%` }} aria-label={`${routine.name} · ${timestamp(routine.next_due_ms)} · Next scheduled occurrence`} onClick={() => select(routine.id)}><span className="work-mark scheduled" /></button></TooltipTrigger><TooltipContent>{timestamp(routine.next_due_ms)}</TooltipContent></Tooltip> : <span className="work-lane-note">{routine.enabled ? "Next occurrence outside this range" : "Paused"}</span>}</div>
            <span className="work-caption">{routine.enabled ? "Enabled" : "Paused"}</span>
          </div>;
        })}
        {!routines.length && <p className="work-caption px-4 py-8">No schedules assigned to this view.</p>}
      </div></div>
    </Tabs>
    {current ? <section id="schedule-inspection" className="work-inspection"><div className="work-heading"><h3 id="schedule-inspection-title" tabIndex={-1}>{current.name}</h3><Button variant="ghost" size="sm" onClick={() => select(null)}>Close details</Button></div><ProfileRoutine routine={current} controls={controls} /></section> : <div className="work-timeline-hint"><Clock size={16} /><p>Select a schedule to inspect its timing, enable it, or pause future admissions.</p></div>}
  </div>;
}

import { useEffect, useState, type MouseEvent } from "react";
import { ArrowRight, CaretLeft, CaretRight, Clock, Lightning, Pause, X } from "@phosphor-icons/react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { DAY, TODAY, NOW, date, time, scheduledTimes, statusLabel, type Automation, type Execution } from "./data";

import type { AgentExample } from "./agent-example";

type Props = AgentExample & {
  description?: string;
  enabled: Record<string, boolean>; setEnabled: (id: string, value: boolean) => void;
  selected: string | null; select: (id: string | null) => void; openRun: (id: string, element: HTMLElement) => void;
};
export function AutomationTimeline({ automations, executions, enabled, setEnabled, selected, select, openRun, description = "What starts this agent’s work." }: Props) {
  const [range, setRange] = useState("day");
  const [offset, setOffset] = useState(0);
  const [selectedTime, setSelectedTime] = useState<{ id: string; timestamp: number } | null>(null);
  const span = range === "day" ? DAY : 7 * DAY;
  const start = (range === "day" ? TODAY : TODAY - 3 * DAY) + offset * span;
  const end = start + span;
  const current = automations.find(item => item.id === selected);
  const ticks = Array.from({ length: range === "day" ? 5 : 7 }, (_, i) => start + (range === "day" ? i * DAY / 4 : i * DAY));
  function zoomDay(id: string, day: number) { setRange("day"); setOffset(Math.round((day - TODAY) / DAY)); setSelectedTime(null); select(id); }
  function selectTime(id: string, value: number) { setSelectedTime({ id, timestamp: value }); select(id); }
  useEffect(() => {
    if (!selected) return;
    const frame = requestAnimationFrame(() => {
      const heading = document.getElementById("automation-inspection-title");
      if (!heading?.getClientRects().length) return;
      heading.focus({ preventScroll: true });
      heading.scrollIntoView({ block: "start", behavior: "instant" });
    });
    return () => cancelAnimationFrame(frame);
  }, [selected, selectedTime]);
  return <div className="flex flex-col gap-6">
    <div className="work-heading"><div><h2>Automations</h2><p>{description}</p></div><span className="work-caption">{Object.values(enabled).filter(Boolean).length} enabled · {Object.values(enabled).filter(value => !value).length} paused</span></div>
    <Tabs value={range} onValueChange={value => { setRange(value); setOffset(0); }} className="gap-5">
      <div className="work-toolbar">
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="icon" aria-label="Previous period" onClick={() => setOffset(value => value - 1)}><CaretLeft /></Button>
          <span className="work-period">{range === "day" ? date(start) : `${date(start)} – ${date(end - 1)}`}</span>
          <Button variant="ghost" size="icon" aria-label="Next period" onClick={() => setOffset(value => value + 1)}><CaretRight /></Button>
          {offset !== 0 && <Button size="sm" variant="outline" onClick={() => setOffset(0)}>Today</Button>}
        </div>
        <div className="flex items-center gap-3"><span className="work-caption">Asia/Kolkata</span><TabsList aria-label="Timeline range"><TabsTrigger value="day">Day</TabsTrigger><TabsTrigger value="week">Week</TabsTrigger></TabsList></div>
      </div>
      <TabsContent value={range}>
        <div className="work-timeline-scroll" tabIndex={0} aria-label="Automation timeline, scroll horizontally on small screens">
          <div className={cn("work-timeline", range === "week" && "work-timeline-week")}>
            <div className="work-axis"><span className="work-caption">Automation</span><div className="work-axis-track">
              {ticks.map(tick => <span key={tick} style={{ left: `${(tick - start + (range === "week" ? DAY / 2 : 0)) / span * 100}%` }}>{range === "day" ? tick === end ? "24:00" : time(tick) : date(tick)}</span>)}
            </div><span /></div>
            {automations.map(automation => {
              const active = enabled[automation.id] ?? false;
              const past = executions.filter(run => run.automationId === automation.id && run.started >= start && run.started < end);
              const future = scheduledTimes(automation, Math.max(start, NOW), end);
              return <div key={automation.id} className={cn("work-lane", selected === automation.id && "work-lane-selected")}>
                <button className="work-lane-name" aria-expanded={selected === automation.id} aria-controls="automation-inspection" onClick={() => { setSelectedTime(null); select(selected === automation.id ? null : automation.id); }}>
                  <span>{automation.name}<CaretRight size={12} /></span><small>{automation.rule}</small>
                </button>
                <div className="work-track" style={{ backgroundSize: `${range === "day" ? 25 : 100 / 7}% 100%` }}>
                  <div className={cn("work-track-line", !active && "work-track-paused")} />
                  {NOW >= start && NOW < end && <div className="work-now-line" style={{ left: `${(NOW - start) / span * 100}%` }} />}
                  {range === "day" && past.map(run => <TimelineMark runId={run.id} key={run.id} position={(run.started - start) / span * 100} kind={run.status} square={automation.schedule.kind === "event"} label={`${automation.name} · ${date(run.started)}, ${time(run.started)} · ${statusLabel[run.status]}`} onClick={event => openRun(run.id, event.currentTarget)} />)}
                  {range === "day" && active && future.map(timestamp => <TimelineMark key={timestamp} position={(timestamp - start) / span * 100} kind="scheduled" square={false} selected={selected === automation.id && selectedTime?.id === automation.id && selectedTime.timestamp === timestamp} label={`${automation.name} · ${date(timestamp)}, ${time(timestamp)} · Scheduled`} onClick={() => selectTime(automation.id, timestamp)} />)}
                  {range === "week" && <WeekOccurrences start={start} past={past} future={active ? future : []} name={automation.name} openDay={day => zoomDay(automation.id, day)} />}
                  {range === "day" && automation.schedule.kind === "event" && <span className="work-lane-note" style={{ left: `${NOW >= start && NOW < end ? Math.min(78, (NOW - start) / span * 100 + 5) : 55}%` }}><Lightning size={12} />{active ? "Listening for events" : "Paused"}</span>}
                  {!active && automation.schedule.kind !== "event" && <span className="work-lane-note"><Pause size={12} />Paused</span>}
                </div>
                <Switch checked={active} onCheckedChange={value => setEnabled(automation.id, value)} aria-label={`${active ? "Pause" : "Enable"} ${automation.name}`} />
              </div>;
            })}
            <div className="work-axis work-axis-bottom"><span /><div className="work-axis-track">{NOW >= start && NOW < end && <span className="work-now-label" style={{ left: `${(NOW - start) / span * 100}%` }}>Now · 11:30</span>}</div><span /></div>
          </div>
        </div>
        <div className="work-legend"><span><i className="work-key completed" />Completed</span><span><i className="work-key interrupted" />Interrupted</span><span><i className="work-key scheduled" />Scheduled</span><span className="work-caption work-scroll-hint">Swipe to explore the timeline</span></div>
      </TabsContent>
    </Tabs>
    {current ? <AutomationInspection executions={executions} automation={current} enabled={enabled[current.id] ?? false} selectedTime={selectedTime?.id === current.id ? selectedTime.timestamp : null} close={() => select(null)} openRun={openRun} /> : <div className="work-timeline-hint"><Clock size={16} /><p>Select an occurrence to follow its work. Select an automation to see its instructions and timing.</p></div>}
  </div>;
}
function TimelineMark({ position, kind, square, label, onClick, runId, selected }: { runId?: string; selected?: boolean; position: number; kind: string; square: boolean; label: string; onClick: (event: MouseEvent<HTMLButtonElement>) => void }) {
  return <Tooltip><TooltipTrigger asChild><button data-run-id={runId} className="work-mark-hit" style={{ left: `${position}%` }} aria-label={label} aria-pressed={selected} onClick={onClick}><span className={cn("work-mark", kind, square && "work-mark-square")} /></button></TooltipTrigger><TooltipContent>{label}</TooltipContent></Tooltip>;
}
function AutomationInspection({ executions, automation, enabled, selectedTime, close, openRun }: { executions: Execution[]; automation: Automation; enabled: boolean; selectedTime: number | null; close: () => void; openRun: (id: string, element: HTMLElement) => void }) {
  const upcoming = scheduledTimes(automation, NOW, NOW + 8 * DAY).slice(0, 3);
  const recent = executions.filter(run => run.automationId === automation.id).slice(0, 3);
  return <section id="automation-inspection" className="work-inspection" aria-label={`${automation.name} details`}>
    <div className="work-heading"><div><div className="flex flex-wrap items-center gap-2"><h3 id="automation-inspection-title" tabIndex={-1}>{automation.name}</h3><Badge variant="outline">{enabled ? "Enabled" : "Paused"}</Badge></div><p>{automation.rule}</p></div><Button variant="ghost" size="icon" aria-label="Close automation details" onClick={close}><X /></Button></div>
    <div className="work-automation-detail"><div><h4>Instructions</h4><p>{automation.description}</p></div><div>{selectedTime !== null && enabled && <p className="work-selected-time">Selected: {date(selectedTime)} · {time(selectedTime)}</p>}<h4>{automation.schedule.kind === "event" ? "Trigger" : enabled ? "Next occurrences" : "Schedule paused"}</h4>
      {automation.schedule.kind === "event" ? <p>{automation.schedule.condition}. {enabled ? "There is no predicted run time." : "New events will not start work."}</p> : enabled ? <div className="flex flex-wrap gap-2">{upcoming.map(timestamp => <Badge key={timestamp} variant={selectedTime === timestamp ? "secondary" : "outline"}>{date(timestamp)} · {time(timestamp)}</Badge>)}</div> : <p>Its history stays available. Enable it to resume scheduled work.</p>}
    </div></div>
    {recent.length > 0 && <div className="work-related"><h4>Recent executions</h4><div className="flex flex-wrap gap-2">{recent.map(run => <Button key={run.id} data-run-id={run.id} variant="outline" size="sm" onClick={event => openRun(run.id, event.currentTarget)}><span className={cn("work-key", run.status)} />{date(run.started)}, {time(run.started)}<ArrowRight data-icon="inline-end" /></Button>)}</div></div>}
  </section>;
}

function WeekOccurrences({ start, past, future, name, openDay }: { start: number; past: Execution[]; future: number[]; name: string; openDay: (day: number) => void }) {
  return Array.from({ length: 7 }, (_, index) => {
    const day = start + index * DAY;
    const runs = past.filter(run => run.started >= day && run.started < day + DAY);
    const scheduled = future.filter(timestamp => timestamp >= day && timestamp < day + DAY).length;
    if (!runs.length && !scheduled) return null;
    const completed = runs.filter(run => run.status === "completed").length;
    const interrupted = runs.filter(run => run.status === "interrupted").length;
    const label = `${name} · ${date(day)} · ${runs.length} recorded, ${scheduled} scheduled. Open day`;
    return <Tooltip key={day}><TooltipTrigger asChild><button className="work-week-group" style={{ left: `${index / 7 * 100}%`, width: `${100 / 7}%` }} aria-label={label} onClick={() => openDay(day)}>
      {completed > 0 && <span><i className="work-key completed" />{completed}</span>}{interrupted > 0 && <span><i className="work-key interrupted" />{interrupted}</span>}{scheduled > 0 && <span><i className="work-key scheduled" />{scheduled}</span>}
    </button></TooltipTrigger><TooltipContent>{label}</TooltipContent></Tooltip>;
  });
}

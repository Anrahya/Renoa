import { useRef, useState, type PointerEvent } from "react";
import { CaretDown, CaretRight, Minus, Plus } from "@phosphor-icons/react";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { boundedWindow, issueFor, overlaps, timecode, traceBins, type TimeWindow, type TraceEvent } from "./trace-model";

type Props = { events: TraceEvent[]; total: number; window: TimeWindow; setWindow: (value: TimeWindow) => void; selected: string | null; select: (event: TraceEvent) => void; agentName: string };
export function RunTimeline({ events, total, window, setWindow, selected, select, agentName }: Props) {
  const [expanded, setExpanded] = useState(false);
  const [brush, setBrush] = useState<TimeWindow | null>(null);
  const anchor = useRef<{ pointerId: number; time: number } | null>(null);
  const span = window.end - window.start;
  const bins = traceBins(events, { start: 0, end: total }, 72);
  const max = Math.max(1, ...bins.map(bin => bin.events.length));
  const workers = [...new Set(events.filter(event => event.laneId !== "main").map(event => event.laneId))];
  const view = brush ?? window;
  function zoom(factor: number) { setWindow(boundedWindow((window.start + window.end - span * factor) / 2, span * factor, total)); }
  function pointerTime(event: PointerEvent<HTMLDivElement>) { const bounds = event.currentTarget.getBoundingClientRect(); return Math.max(0, Math.min(total, (event.clientX - bounds.left) / bounds.width * total)); }
  function begin(event: PointerEvent<HTMLDivElement>) { if (event.button !== 0 || anchor.current !== null || !event.isPrimary) return; anchor.current = { pointerId: event.pointerId, time: pointerTime(event) }; event.currentTarget.setPointerCapture(event.pointerId); }
  function move(event: PointerEvent<HTMLDivElement>) { if (anchor.current?.pointerId !== event.pointerId) return; const value = pointerTime(event); setBrush({ start: Math.min(value, anchor.current.time), end: Math.max(value, anchor.current.time) }); }
  function finish(event: PointerEvent<HTMLDivElement>) {
    if (anchor.current?.pointerId !== event.pointerId) return;
    const value = pointerTime(event);
    const distance = Math.abs(value - anchor.current.time);
    setWindow(distance > total * .015 ? boundedWindow(Math.min(value, anchor.current.time), distance, total) : boundedWindow(value - span / 2, span, total));
    anchor.current = null; setBrush(null);
  }
  function cancel(event: PointerEvent<HTMLDivElement>) { if (anchor.current?.pointerId === event.pointerId) { anchor.current = null; setBrush(null); } }
  return <section className="run-timeline" aria-labelledby="run-timeline-title">
    <div className="run-section-heading"><div><h2 id="run-timeline-title" tabIndex={-1}>Execution timeline</h2><p>{events.length} events on a shared clock</p></div><div className="run-zoom-controls"><Button variant="ghost" size="sm" onClick={() => setWindow({ start: 0, end: total })}>Fit all</Button><Button variant="outline" size="icon" aria-label="Zoom out" disabled={span >= total} onClick={() => zoom(2)}><Minus /></Button><Button variant="outline" size="icon" aria-label="Zoom in" disabled={span <= 1} onClick={() => zoom(.5)}><Plus /></Button></div></div>
    <div className="run-overview" role="group" aria-label="Whole execution. Drag to select a time range." onPointerDown={begin} onPointerMove={move} onPointerUp={finish} onPointerCancel={cancel} onLostPointerCapture={cancel}>
      <svg viewBox="0 0 720 40" preserveAspectRatio="none" aria-hidden="true">{bins.map((bin, index) => <rect key={index} className={bin.events.some(event => issueFor(event) && !issueFor(event)!.recovered) ? "failed" : bin.events.some(event => issueFor(event)) ? "recovered" : undefined} x={index * 10 + 1} y={38 - bin.events.length / max * 30} width="7" height={Math.max(2, bin.events.length / max * 30)} rx="1" />)}</svg>
      <div className="run-brush" style={{ left: `${view.start / total * 100}%`, width: `${(view.end - view.start) / total * 100}%` }}><span /><span /></div>
    </div>
    <div className="run-overview-caption"><span>00:00</span><span>Drag to zoom · {timecode(window.start)}–{timecode(window.end)} selected</span><span>{timecode(total)}</span></div>
    {span < total && <label className="run-pan"><span>Move window</span><input type="range" min="0" max={total - span} step={Math.min(1, span / 10)} value={window.start} onChange={event => setWindow(boundedWindow(Number(event.target.value), span, total))} aria-label="Move visible time window" aria-valuetext={`${timecode(window.start)} to ${timecode(window.end)}`} /></label>}
    <div className="run-tracks">
      <div className="run-track-axis"><span>Agent</span><div>{Array.from({ length: 5 }, (_, i) => <span key={i} style={{ left: `${i * 25}%` }}>{timecode(window.start + span * i / 4)}</span>)}</div></div>
      <Track label={agentName} events={events.filter(event => event.laneId === "main")} {...{ window, selected, select, setWindow }} />
      {workers.length > 0 && <><div className="run-track-row"><button className="run-worker-toggle" aria-expanded={expanded} onClick={() => setExpanded(value => !value)}>{expanded ? <CaretDown size={14} /> : <CaretRight size={14} />}{workers.length} subagents</button><div className="run-track-summary">{!expanded && <TrackMarks events={events.filter(event => event.laneId !== "main")} {...{ window, selected, select, setWindow }} aggregate />}</div></div>
        {expanded && workers.map((lane, index) => <Track key={lane} label={`Worker ${String.fromCharCode(65 + index)}`} events={events.filter(event => event.laneId === lane)} {...{ window, selected, select, setWindow }} />)}</>}
    </div>
    <p className="run-density-note">{events.filter(event => overlaps(event, window)).length > 28 ? "Bars group nearby events. Select a bar to zoom in." : "Select a call to inspect its input and output."}</p>
    <div className="work-legend"><span><i className="work-key model" />Model request</span><span><i className="work-key tool" />Tool</span><span><i className="work-key waiting" />Recovered error</span><span><i className="work-key interrupted" />Interruption</span></div>
  </section>;
}
function Track({ label, ...props }: Omit<Props, "total" | "agentName"> & { label: string }) {
  return <div className="run-track-row"><span className="run-track-label">{label}</span><div className="run-track-bars"><TrackMarks {...props} /></div></div>;
}
function TrackMarks({ events, window, selected, select, setWindow, aggregate = false }: Omit<Props, "total" | "agentName"> & { aggregate?: boolean }) {
  const shown = events.filter(event => overlaps(event, window));
  const span = window.end - window.start;
  if (aggregate || shown.length > 28) return <>{traceBins(shown, window, 48, true).map((bin, index) => {
    if (!bin.events.length) return null;
    const issues = bin.events.map(issueFor).filter(Boolean);
    return <Tooltip key={index}><TooltipTrigger asChild><button className={cn("run-density-bin", issues.some(issue => !issue!.recovered) ? "failed" : issues.length > 0 ? "recovered" : "")} style={{ left: `${index / 48 * 100}%`, width: `${100 / 48}%`, height: `${Math.min(27, 8 + bin.events.length * 4)}px` }} aria-label={`${timecode(bin.start)}: ${bin.events.length} events. Zoom into interval`} onClick={() => setWindow(boundedWindow(bin.start, span / 48, window.end))} /></TooltipTrigger><TooltipContent>{timecode(bin.start)} · {bin.events.length} events · Select to zoom</TooltipContent></Tooltip>;
  })}</>;
  return <>{shown.map(event => {
    const start = Math.max(window.start, event.start), end = Math.min(window.end, event.end), issue = issueFor(event);
    return <Tooltip key={event.id}><TooltipTrigger asChild><button className={cn("run-span", event.kind, issue && (issue.recovered ? "recovered" : "failed"), selected === event.id && "selected")} style={{ left: `${(start - window.start) / span * 100}%`, width: `${(end - start) / span * 100}%` }} aria-label={`${event.name} at ${timecode(event.start)}${issue ? issue.recovered ? ", recovered error" : ", interrupted" : ""}`} aria-pressed={selected === event.id} onClick={() => select(event)} /></TooltipTrigger><TooltipContent>{event.name} · {timecode(event.start)}{issue ? ` · ${issue.recovered ? "Recovered" : "Interrupted"}` : ""}</TooltipContent></Tooltip>;
  })}</>;
}

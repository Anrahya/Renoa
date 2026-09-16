import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowLeft, ArrowRight, CaretLeft, CaretRight, WarningCircle } from "@phosphor-icons/react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Checkbox } from "@/components/ui/checkbox";
import { cn } from "@/lib/utils";
import { date, duration, statusLabel, time, type Execution } from "./data";
import { boundedWindow, findEvents, issueFor, overlaps, timecode, traceDuration, traceEvents, type TimeWindow, type TraceEvent } from "./trace-model";
import { EventInspector } from "./run-event-inspector";
import { RunTimeline } from "./run-timeline";
const PAGE_SIZE = 10;
type Props = { run: Execution; agentName: string; backLabel: string; back: () => void; openAutomation: (id: string) => void };
export function RunPage({ run, agentName, backLabel, back, openAutomation }: Props) {
  const events = useMemo(() => traceEvents(run), [run]);
  const total = traceDuration(events);
  const interruption = [...events].reverse().find(event => { const issue = issueFor(event); return issue && !issue.recovered; });
  const errorCount = events.filter(event => issueFor(event)).length;
  const recovered = events.filter(event => issueFor(event)?.recovered).length;
  const [window, setWindow] = useState<TimeWindow>(() => interruption ? boundedWindow(interruption.start - 20, Math.max(90, interruption.seconds * 2), Math.max(1, total)) : { start: 0, end: Math.max(1, total) });
  const [selected, setSelected] = useState<string | null>(interruption?.id ?? events[0]?.id ?? null);
  const [query, setQuery] = useState("");
  const [issuesOnly, setIssuesOnly] = useState(false);
  const [page, setPage] = useState(0);
  const heading = useRef<HTMLHeadingElement>(null);
  const matches = findEvents(events, window, query, issuesOnly);
  const lastPage = Math.max(0, Math.ceil(matches.length / PAGE_SIZE) - 1);
  const currentPage = Math.min(page, lastPage);
  const shown = matches.slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE);
  const selectedEvent = events.find(event => event.id === selected);
  useEffect(() => { heading.current?.focus({ preventScroll: true }); globalThis.scrollTo({ top: 0 }); }, []);
  function changeWindow(next: TimeWindow) { setWindow(next); setPage(0); setSelected(null); }
  function select(event: TraceEvent) {
    setSelected(event.id);
    const next = overlaps(event, window) ? window : boundedWindow(event.start - 10, Math.max(60, event.seconds * 3), Math.max(1, total));
    setWindow(next);
    const index = findEvents(events, next, query, issuesOnly).findIndex(item => item.id === event.id);
    setPage(Math.max(0, Math.floor(index / PAGE_SIZE)));
  }
  function jump(event: TraceEvent) { setQuery(""); setIssuesOnly(false); changeWindow(boundedWindow(event.start - 20, Math.max(90, event.seconds * 2), Math.max(1, total))); setSelected(event.id); requestAnimationFrame(() => { const target = document.getElementById("run-timeline-title"); target?.focus({ preventScroll: true }); target?.scrollIntoView({ block: "start" }); }); }
  return <div className="run-page">
    <Button variant="ghost" size="sm" className="w-fit" onClick={back}><ArrowLeft data-icon="inline-start" />Back to {backLabel}</Button>
    <header className="run-header"><div className="flex flex-wrap items-center gap-2"><span className="work-caption">{agentName} / Execution</span><Badge variant={run.status === "interrupted" ? "destructive" : "outline"}>{statusLabel[run.status]}</Badge></div><h1 ref={heading} tabIndex={-1}>{run.title}</h1><div className="run-meta"><span>{date(run.started)} · {time(run.started)}–{time(run.started + total * 1000)}</span><span>{duration(total)}</span><span>{run.source}</span>{run.automationId && <button onClick={() => openAutomation(run.automationId!)}>View automation <ArrowRight size={13} /></button>}</div><p className="run-outcome">{run.result}</p>
      <details className="run-request"><summary>Request & usage</summary><p>{run.input}</p>{run.usage && <p className="work-caption">{run.usage.input.toLocaleString()} input tokens · {run.usage.output.toLocaleString()} output tokens</p>}</details>
    </header>
    {interruption && <div className="run-interruption"><WarningCircle size={20} /><div><strong>{issueFor(interruption)!.origin} interruption at {timecode(interruption.start)}</strong><p>{issueFor(interruption)!.message}</p></div><Button variant="outline" size="sm" onClick={() => jump(interruption)}>Jump to interruption<ArrowRight data-icon="inline-end" /></Button></div>}
    {events.length > 0 ? <>
      <RunTimeline {...{ events, window, selected, select, agentName }} total={Math.max(1, total)} setWindow={changeWindow} />
      <section className="run-records" aria-labelledby="run-records-title">
        <div className="run-section-heading"><div><h2 id="run-records-title">Recorded events</h2><p>{query.trim() ? "Searching the whole execution" : `${timecode(window.start)}–${timecode(window.end)} selected`} · {matches.length} {matches.length === 1 ? "event" : "events"}</p></div>{recovered > 0 && <Button variant="ghost" size="sm" onClick={() => { setIssuesOnly(true); setQuery(""); changeWindow({ start: 0, end: total }); }}>{errorCount} errors · {recovered} recovered<ArrowRight data-icon="inline-end" /></Button>}</div>
        <div className="run-record-filters"><label className="run-search"><span className="sr-only">Search all recorded events</span><Input placeholder="Search calls, output or error code…" value={query} onChange={event => { setQuery(event.target.value); setPage(0); }} /></label><label className="run-issues-filter"><Checkbox checked={issuesOnly} onCheckedChange={value => { setIssuesOnly(value === true); setPage(0); }} />Errors only</label></div>
        <div className="run-inspection-grid"><div className="run-event-browser"><div className="run-event-table" aria-label="Recorded events in selection">{shown.length ? shown.map(event => {
          const issue = issueFor(event);
          return <button key={event.id} className={cn("run-event-row", selected === event.id && "selected")} aria-pressed={selected === event.id} onClick={() => select(event)}><span className="run-event-time">{timecode(event.start)}</span><i className={cn("work-key", issue ? issue.recovered ? "waiting" : "interrupted" : event.kind)} /><span className="run-event-name"><strong>{event.name}</strong><small>{event.laneId === "main" ? agentName : event.laneId === "worker-a" ? "Worker A" : "Worker B"}{issue && ` · ${issue.recovered ? "Recovered" : "Interrupted"}`}</small></span><span className="run-event-duration">{duration(event.seconds)}</span></button>;
        }) : <div className="run-empty"><p>No events match this selection.</p><Button variant="outline" size="sm" onClick={() => { setQuery(""); setIssuesOnly(false); changeWindow({ start: 0, end: total }); }}>Show all events</Button></div>}</div>
          <div className="run-pagination"><span>{matches.length ? `${currentPage * PAGE_SIZE + 1}–${Math.min((currentPage + 1) * PAGE_SIZE, matches.length)} of ${matches.length}` : "0 events"}</span><div className="flex gap-1"><Button variant="ghost" size="icon" aria-label="Previous events" disabled={currentPage === 0} onClick={() => setPage(currentPage - 1)}><CaretLeft /></Button><Button variant="ghost" size="icon" aria-label="Next events" disabled={currentPage === lastPage} onClick={() => setPage(currentPage + 1)}><CaretRight /></Button></div></div>
        </div><EventInspector event={selectedEvent} agentName={agentName} /></div>
      </section>
    </> : <p className="run-empty">No event records are available for this execution.</p>}
  </div>;
}

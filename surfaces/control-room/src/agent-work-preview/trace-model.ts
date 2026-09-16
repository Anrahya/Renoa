import type { Execution, ExecutionEvent } from "./data";
export type TraceEvent = ExecutionEvent & { start: number; end: number; laneId: string };
export type TimeWindow = { start: number; end: number };
export function traceEvents(run: Execution): TraceEvent[] {
  let cursor = 0;
  return run.events.map(event => {
    const start = event.offset ?? cursor;
    const end = start + event.seconds;
    cursor = end;
    return { ...event, start, end, laneId: event.lane ?? "main" };
  }).sort((a, b) => a.start - b.start || a.id.localeCompare(b.id));
}
export const traceDuration = (events: TraceEvent[]) => Math.max(0, ...events.map(event => event.end));
export const overlaps = (event: TraceEvent, window: TimeWindow) => event.end > window.start && event.start < window.end;
export function boundedWindow(start: number, width: number, total: number): TimeWindow {
  const span = Math.min(total, Math.max(Math.min(1, total), width));
  const left = Math.max(0, Math.min(start, total - span));
  return { start: left, end: left + span };
}
export const timecode = (seconds: number) => `${Math.floor(seconds / 60).toString().padStart(2, "0")}:${Math.floor(seconds % 60).toString().padStart(2, "0")}`;
export function issueFor(event: ExecutionEvent) {
  return event.issue ?? (event.failed ? { code: "", origin: "Tool", recovered: false, message: event.output } : undefined);
}
export function traceBins(events: TraceEvent[], window: TimeWindow, count: number, occupancy = false) {
  const width = (window.end - window.start) / count;
  return Array.from({ length: count }, (_, index) => {
    const start = window.start + index * width;
    const end = start + width;
    return { start, end, events: events.filter(event => occupancy ? overlaps(event, { start, end }) : event.start >= start && event.start < end) };
  });
}
export function findEvents(events: TraceEvent[], window: TimeWindow, query: string, issuesOnly: boolean) {
  const term = query.trim().toLowerCase();
  return events.filter(event => (term ? `${event.name} ${event.laneId} ${event.input} ${event.output} ${issueFor(event)?.code ?? ""}`.toLowerCase().includes(term) : overlaps(event, window)) && (!issuesOnly || issueFor(event)));
}

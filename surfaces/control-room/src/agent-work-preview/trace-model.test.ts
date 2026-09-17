import { describe, expect, it } from "vitest";
import { executions } from "./example-data";
import { boundedWindow, findEvents, issueFor, overlaps, traceBins, traceDuration, traceEvents } from "./trace-model";
const longRun = executions.find(run => run.id === "run-107")!;
const events = traceEvents(longRun);
describe("execution inspection", () => {
  it("puts parallel work on a shared clock instead of adding call durations", () => {
    expect(events).toHaveLength(200);
    expect(new Set(events.map(event => event.laneId)).size).toBe(3);
    expect(traceDuration(events)).toBe(3600);
    expect(events.filter(event => event.start >= 900 && event.start < 960).map(event => event.laneId)).toEqual(expect.arrayContaining(["main", "worker-a", "worker-b"]));
    expect(traceDuration(events)).not.toBe(events.reduce((sum, event) => sum + event.seconds, 0));
  });
  it("retains a call crossing a zoom boundary without double counting event starts", () => {
    const last = events.find(event => event.id === "main-79")!;
    expect(overlaps(last, { start: 3570, end: 3600 })).toBe(true);
    expect(overlaps(last, { start: 3600, end: 3660 })).toBe(false);
    const counts = traceBins(events, { start: 0, end: 3600 }, 72).reduce((sum, bin) => sum + bin.events.length, 0);
    expect(counts).toBe(200);
    expect(traceBins([last], { start: 3570, end: 3600 }, 2, true).map(bin => bin.events.length)).toEqual([1, 1]);
  });
  it("keeps zoom and panning inside the run", () => {
    expect(boundedWindow(-30, 120, 3600)).toEqual({ start: 0, end: 120 });
    expect(boundedWindow(3570, 120, 3600)).toEqual({ start: 3480, end: 3600 });
    expect(boundedWindow(60, 7200, 3600)).toEqual({ start: 0, end: 3600 });
    expect(boundedWindow(0, 0, 0)).toEqual({ start: 0, end: 0 });
  });
  it("finds an error outside the selected range and separates recovered failures", () => {
    const window = { start: 0, end: 60 };
    expect(findEvents(events, window, "", true)).toEqual([]);
    const match = findEvents(events, window, "service_unavailable", true);
    expect(match.map(event => event.id)).toEqual(["main-79"]);
    const errors = findEvents(events, { start: 0, end: 3600 }, "", true);
    expect(errors).toHaveLength(3);
    expect(errors.filter(event => issueFor(event)?.recovered)).toHaveLength(2);
    expect(findEvents(events, window, "nothing-matches-this", false)).toEqual([]);
  });
});

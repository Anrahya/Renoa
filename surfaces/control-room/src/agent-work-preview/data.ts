import { traceDuration, traceEvents } from "./trace-model";

// Shared helpers, anchors and record types for the work views. The example records
// live in example-data.ts, so production views import these without the fixtures.
export const DAY = 24 * 60 * 60 * 1000;
export const TODAY = Date.parse("2026-09-17T00:00:00+05:30");
export const NOW = TODAY + 11.5 * 60 * 60 * 1000;
export const atHour = (hour: number) => TODAY + hour * 60 * 60 * 1000;
export const time = (ms: number) => new Intl.DateTimeFormat("en-GB", { timeZone: "Asia/Kolkata", hour: "2-digit", minute: "2-digit" }).format(ms);
export const date = (ms: number) => new Intl.DateTimeFormat("en-GB", { timeZone: "Asia/Kolkata", day: "numeric", month: "short" }).format(ms);
export const duration = (seconds: number) => seconds < 60 ? `${seconds.toFixed(seconds % 1 ? 1 : 0)}s` : `${Math.floor(seconds / 60)}m ${Math.round(seconds % 60)}s`;

export type ExecutionEvent = {
  id: string; kind: "model" | "tool" | "wait"; name: string; seconds: number;
  input: string; output: string; failed?: boolean; offset?: number; lane?: string;
  issue?: { code: string; origin: string; recovered: boolean; message: string };
};
export type Execution = {
  id: string; title: string; source: string; automationId?: string;
  started: number; status: "completed" | "interrupted" | "waiting";
  input: string; result: string; events: ExecutionEvent[];
  usage?: { input: number; output: number };
};
export type Automation = {
  id: string; name: string; rule: string; description: string; enabled: boolean;
  schedule: { kind: "daily"; hours: number[] } | { kind: "weekly"; day: number; hour: number } | { kind: "event"; condition: string };
};
export const statusLabel = { completed: "Completed", interrupted: "Interrupted", waiting: "Needs your input" };
export const totalSeconds = (run: Execution) => traceDuration(traceEvents(run));
export function scheduledTimes(automation: Automation, start: number, end: number): number[] {
  const schedule = automation.schedule;
  if (schedule.kind === "event") return [];
  const result: number[] = [];
  for (let day = Math.floor((start - TODAY) / DAY); TODAY + day * DAY < end; day++) {
    const hours = schedule.kind === "daily" ? schedule.hours : ((day - schedule.day) % 7 === 0 ? [schedule.hour] : []);
    for (const hour of hours) {
      const timestamp = TODAY + day * DAY + hour * DAY / 24;
      if (timestamp >= start && timestamp < end) result.push(timestamp);
    }
  }
  return result;
}

import { longExecution } from "./long-run";
import { traceDuration, traceEvents } from "./trace-model";

// Design fixtures only. These are view data, not a proposed Host or kernel contract.
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
export const automations: Automation[] = [
  { id: "brief", name: "Morning briefing", rule: "Every day · 09:00", description: "Bring together the updates from my selected sources. Keep the summary brief and link back to anything that needs attention.", enabled: true, schedule: { kind: "daily", hours: [9] } },
  { id: "workspace", name: "Workspace check", rule: "Every 6 hours", description: "Check the assigned workspace and report anything that needs my attention. Leave files unchanged.", enabled: true, schedule: { kind: "daily", hours: [2, 8, 14, 20] } },
  { id: "event", name: "Incoming event", rule: "When an event arrives", description: "Read the incoming event and decide whether it needs a response using my instructions.", enabled: true, schedule: { kind: "event", condition: "A matching event arrives from a connected source" } },
  { id: "cleanup", name: "Weekly cleanup", rule: "Sundays · 18:00", description: "Review temporary workspace files and propose what can be removed. Ask before deleting anything.", enabled: false, schedule: { kind: "weekly", day: 3, hour: 18 } },
];
const model = (id: string, seconds: number, output: string): ExecutionEvent => ({ id, kind: "model", name: "Model response", seconds, input: "Agent instructions and the recorded context for this execution.", output });
const tool = (id: string, name: string, seconds: number, input: string, output: string, failed = false): ExecutionEvent => ({ id, kind: "tool", name, seconds, input, output, failed });
export const executions: Execution[] = [
  longExecution(atHour(10.25)),
  { id: "run-106", title: "Review the proposed changes", source: "Direct message", started: atHour(11.2), status: "waiting", input: "Review the changes and tell me if anything needs my attention.", result: "The review is ready. Should I apply the two suggested changes?", usage: { input: 3200, output: 620 }, events: [
    model("m1", 3.2, "I’ll read the changes and check the surrounding context."),
    tool("t1", "workspace.read", 1.4, '{ "path": "changes.diff" }', "2 files changed. 38 lines added, 12 lines removed."),
    model("m2", 9.2, "The review is ready. Should I apply the two suggested changes?") ] },
  { id: "run-105", title: "Incoming event", source: "Event trigger", automationId: "event", started: atHour(10.3), status: "completed", input: "Inspect the incoming event using the agent’s configured instructions.", result: "No action needed. The update is already covered by the previous response.", usage: { input: 1850, output: 210 }, events: [
    model("m1", 2.4, "I’ll compare this event with the previous update."),
    tool("t1", "context.read", 0.8, '{ "reference": "previous-update" }', "The previous update covers the same change."),
    model("m2", 4.1, "No action needed. The update is already covered by the previous response.") ] },
  { id: "run-104", title: "Morning briefing", source: "Schedule · 09:00", automationId: "brief", started: atHour(9), status: "completed", input: automations[0]!.description, result: "Your briefing is ready. Two updates need a look; everything else can wait.", usage: { input: 8400, output: 1140 }, events: [
    model("m1", 4.2, "I’ll check the selected sources and pull together what changed."),
    tool("t1", "sources.list", 1.6, '{ "scope": "selected" }', "3 sources are selected for this agent."),
    model("m2", 2.8, "I’ll read the recent updates from these sources."),
    tool("t2", "sources.read", 8.5, '{ "since": "previous-run" }', "6 updates returned from the selected sources."),
    model("m3", 12.3, "Your briefing is ready. Two updates need a look; everything else can wait.") ] },
  { id: "run-103", title: "Workspace check", source: "Schedule · 08:00", automationId: "workspace", started: atHour(8), status: "interrupted", input: automations[1]!.description, result: "The workspace connection timed out. No changes were made.", usage: { input: 960, output: 140 }, events: [
    model("m1", 2.1, "I’ll inspect the assigned workspace."),
    tool("t1", "workspace.inspect", 30, '{ "workspace": "assigned", "read_only": true }', "Connection timed out after 30 seconds. No result was returned.", true) ] },
  { id: "run-102", title: "Workspace check", source: "Schedule · 02:00", automationId: "workspace", started: atHour(2), status: "completed", input: automations[1]!.description, result: "Workspace checked. Nothing needs your attention.", usage: { input: 1220, output: 190 }, events: [
    model("m1", 2, "I’ll inspect the assigned workspace."),
    tool("t1", "workspace.inspect", 1.5, '{ "workspace": "assigned", "read_only": true }', "Workspace available. No pending changes."),
    model("m2", 3.8, "Workspace checked. Nothing needs your attention.") ] },
  { id: "run-101", title: "Morning briefing", source: "Schedule · 09:00", automationId: "brief", started: atHour(9) - DAY, status: "completed", input: automations[0]!.description, result: "Three updates summarized. No follow-up needed.", events: [model("m1", 5, "Three updates summarized. No follow-up needed.")] },
];
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

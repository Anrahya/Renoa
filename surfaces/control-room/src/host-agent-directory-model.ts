import type { Agent, HostSnapshot } from "./host-contract";
import { agentOverview } from "./host-agent-overview";
import { agentHref, scheduleText, timestamp } from "./host-presentation";
import { toneState, type AgentState, type DayMark } from "./host-state";
import type { AgentExample } from "./agent-work-preview/agent-example";
import { DAY, NOW, TODAY, scheduledTimes, statusLabel, time, date } from "./agent-work-preview/data";

export type DirectoryTone = "interrupted" | "waiting" | "pending" | "completed" | "quiet";
export type DirectorySummary = {
  tone: DirectoryTone; status: string; title: string; detail: string; workHref: string;
  automated: boolean; automationCount: number;
  next: { title: string; detail: string; href: string };
  /** Today's recorded work, oldest first. Empty when the Host recorded none. */
  day: DayMark[];
  /** When the most recent record landed, for relative time on attention rows. */
  lastAt: number | null;
};

export function directorySummary(host: HostSnapshot, agent: Agent, example?: AgentExample): DirectorySummary {
  if (example) return exampleSummary(agent.id, example);
  const data = agentOverview(host, agent);
  const next = data.scheduled[0];
  const failed = data.sessions.some(session => session.observation === "available" && (session.active_operation ?? session.latest_operation)?.state === "failed");
  const tone = data.activity.tone === "attention" ? failed ? "interrupted" : "waiting" : data.activity.tone === "pending" ? "pending" : "quiet";
  return {
    tone, status: tone === "interrupted" || tone === "waiting" ? "Needs attention" : tone === "pending" ? "Unfinished work" : "No unfinished work",
    title: data.activity.label,
    detail: tone !== "quiet" ? "Open activity to inspect the retained work and diagnostics." : "Based on the Host’s retained records.",
    workHref: agentHref(agent.id, "activity"),
    automated: data.routines.length > 0,
    automationCount: data.routines.length,
    // The live snapshot records no timestamped work, so nothing reaches the day axis.
    day: [], lastAt: null,
    next: {
      title: next?.name ?? (data.paused.length ? "Schedules paused" : "No scheduled work"),
      detail: next ? `${timestamp(next.next_due_ms)} · ${scheduleText(next)}` : data.paused.length ? `${data.paused.length} paused` : "Starts when you assign work.",
      href: agentHref(agent.id, "automations"),
    },
  };
}

const runState = (status: string): AgentState =>
  toneState(status === "waiting" ? "waiting" : status === "interrupted" ? "interrupted" : "completed");

function exampleSummary(agentId: string, example: AgentExample): DirectorySummary {
  const today = example.executions.filter(run => run.started >= TODAY && run.started < TODAY + DAY).sort((a, b) => b.started - a.started);
  const interrupted = today.filter(run => run.status === "interrupted");
  const waiting = today.filter(run => run.status === "waiting");
  const focus = interrupted[0] ?? waiting[0] ?? today[0];
  const upcoming = example.automations.filter(item => item.enabled).flatMap(item => scheduledTimes(item, NOW, NOW + 8 * DAY).map(at => ({ item, at }))).sort((a, b) => a.at - b.at)[0];
  const listeners = example.automations.filter(item => item.enabled && item.schedule.kind === "event").length;
  const paused = example.automations.filter(item => !item.enabled).length;
  return {
    tone: focus?.status ?? "quiet", status: focus ? statusLabel[focus.status] : "No activity today",
    title: focus?.title ?? "No work recorded today",
    detail: focus?.result ?? "New activity will appear here.",
    workHref: focus ? `${agentHref(agentId, "activity")}/${encodeURIComponent(focus.id)}` : agentHref(agentId, "activity"),
    automated: example.automations.length > 0, automationCount: example.automations.length,
    day: [...today].reverse().map(run => ({ at: run.started, state: runState(run.status) })),
    lastAt: today[0]?.started ?? null,
    next: {
      title: upcoming ? `${upcoming.at >= TODAY + DAY ? `${date(upcoming.at)} · ` : ""}${time(upcoming.at)} · ${upcoming.item.name}` : listeners ? "On incoming events" : paused ? "Automations paused" : "No scheduled work",
      detail: upcoming ? upcoming.item.rule : listeners ? `${listeners} event ${listeners === 1 ? "trigger" : "triggers"} enabled` : paused ? `${paused} paused · History is kept` : "Starts when you assign work.",
      href: agentHref(agentId, "automations"),
    },
  };
}

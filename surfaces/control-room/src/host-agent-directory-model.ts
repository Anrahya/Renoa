import type { Agent, HostSnapshot } from "./host-contract";
import { agentOverview } from "./host-agent-overview";
import { agentHref, attentionReviews, scheduleText, timestamp } from "./host-presentation";
import type { AgentExample } from "./agent-work-preview/agent-example";
import { DAY, NOW, TODAY, scheduledTimes, statusLabel, time, date } from "./agent-work-preview/data";

export type DirectoryTone = "interrupted" | "waiting" | "pending" | "completed" | "quiet";
export type DirectorySummary = {
  tone: DirectoryTone; status: string; title: string; detail: string; workHref: string;
  automated: boolean; automationCount: number;
  next: { title: string; detail: string; href: string };
  day?: { bins: DirectoryTone[]; completed: number; attention: number; total: number };
};

export function directorySummary(host: HostSnapshot, agent: Agent, example?: AgentExample): DirectorySummary {
  if (example) return exampleSummary(agent.id, example);
  const data = agentOverview(host, agent);
  const attention = attentionReviews(data.reviews)[0];
  const latest = data.latestAdmission;
  const next = data.scheduled[0];
  const listeners = data.repositories.filter(item => item.policy.enabled).length;
  const tone = data.activity.tone === "attention" ? "interrupted" : data.activity.tone === "pending" ? "pending" : "quiet";
  return {
    tone, status: tone === "interrupted" ? "Needs attention" : tone === "pending" ? "Unfinished work" : "No unfinished work",
    title: attention ? `${attention.repository} #${attention.pull_number}` : tone !== "quiet" ? data.activity.label : latest ? `${latest.repository} #${latest.pull_number}` : data.activity.label,
    detail: attention ? `${attention.state} · ${data.activity.label}` : tone !== "quiet" ? "Open activity to inspect the retained work and diagnostics." : latest ? `Last review admitted ${timestamp(latest.admitted_at_ms)} · ${latest.state}` : "Based on the Host’s retained records.",
    workHref: agentHref(agent.id, "activity"),
    automated: data.routines.length + data.repositories.length > 0,
    automationCount: data.routines.length + data.repositories.length,
    next: {
      title: next?.name ?? (listeners ? "On repository events" : data.paused.length ? "Schedules paused" : "No scheduled work"),
      detail: next ? `${timestamp(next.next_due_ms)} · ${scheduleText(next)}` : listeners ? `${listeners} ${listeners === 1 ? "repository" : "repositories"} enabled` : data.paused.length ? `${data.paused.length} paused` : "Starts when you assign work.",
      href: agentHref(agent.id, "automations"),
    },
  };
}

function exampleSummary(agentId: string, example: AgentExample): DirectorySummary {
  const today = example.executions.filter(run => run.started >= TODAY && run.started < TODAY + DAY).sort((a, b) => b.started - a.started);
  const interrupted = today.filter(run => run.status === "interrupted");
  const waiting = today.filter(run => run.status === "waiting");
  const focus = interrupted[0] ?? waiting[0] ?? today[0];
  const completed = today.filter(run => run.status === "completed").length;
  const upcoming = example.automations.filter(item => item.enabled).flatMap(item => scheduledTimes(item, NOW, NOW + 8 * DAY).map(at => ({ item, at }))).sort((a, b) => a.at - b.at)[0];
  const listeners = example.automations.filter(item => item.enabled && item.schedule.kind === "event").length;
  const paused = example.automations.filter(item => !item.enabled).length;
  const bins = Array.from({ length: 24 }, (_, hour): DirectoryTone => {
    const runs = today.filter(run => Math.floor((run.started - TODAY) / (DAY / 24)) === hour);
    return runs.some(run => run.status === "interrupted") ? "interrupted" : runs.some(run => run.status === "waiting") ? "waiting" : runs.length ? "completed" : "quiet";
  });
  return {
    tone: focus?.status ?? "quiet", status: focus ? statusLabel[focus.status] : "No activity today",
    title: focus?.title ?? "No work recorded today",
    detail: focus?.result ?? "New activity will appear here.",
    workHref: focus ? `${agentHref(agentId, "activity")}/${encodeURIComponent(focus.id)}` : agentHref(agentId, "activity"),
    automated: example.automations.length > 0, automationCount: example.automations.length,
    next: {
      title: upcoming ? `${upcoming.at >= TODAY + DAY ? `${date(upcoming.at)} · ` : ""}${time(upcoming.at)} · ${upcoming.item.name}` : listeners ? "On incoming events" : paused ? "Automations paused" : "No scheduled work",
      detail: upcoming ? upcoming.item.rule : listeners ? `${listeners} event ${listeners === 1 ? "trigger" : "triggers"} enabled` : paused ? `${paused} paused · History is kept` : "Starts when you assign work.",
      href: agentHref(agentId, "automations"),
    },
    day: { bins, completed, attention: interrupted.length + waiting.length, total: today.length },
  };
}

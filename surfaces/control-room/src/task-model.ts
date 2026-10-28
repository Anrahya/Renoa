import type { TaskEvent, TaskStatus, TaskSummary } from "@renoa/rcp-client/browser";

const TITLE_WORDS: Readonly<Record<string, string>> = {
  Api: "API",
  Acp: "ACP",
  Github: "GitHub",
  Mcp: "MCP",
  Rcp: "RCP",
  Vps: "VPS",
};

export type TaskState = "ready" | "queued" | "working" | "failed" | "cancelled";

export interface TaskView {
  readonly taskId: string;
  readonly target: string;
  readonly title: string;
  readonly state: TaskState;
  readonly detail: string;
  readonly lastRecordedAtMs: number | null;
  readonly eventCount: number;
}

// What a task shows while the Control Room is not attached to it: the status
// the directory reports, because a journal saved earlier may be out of date.
// An attached task's journal is current, and the projection below names finer
// states of its own ("queued", a failed tool call) that are not RCP statuses.
const DIRECTORY_VIEW: Readonly<Record<TaskStatus, { readonly state: TaskState; readonly detail: string }>> = {
  working: { state: "working", detail: "Agent is working" },
  idle: { state: "ready", detail: "Ready to continue" },
  failed: { state: "failed", detail: "The last command failed" },
};

export function projectTask(
  task: TaskSummary,
  events: readonly TaskEvent[],
  attached: boolean,
): TaskView {
  let state: TaskState = "ready";
  let detail = events.length === 0 ? "No activity recorded" : "Ready to continue";
  let lastRecordedAtMs: number | null = null;
  const toolNames = new Map<string, string>();

  for (const record of events) {
    if (record.kind.type === "command_submitted") {
      state = "queued";
      detail = "Command admitted";
      continue;
    }
    const event = record.kind.event;
    lastRecordedAtMs = event.recordedAtMs;
    switch (event.kind.type) {
      case "execution_started":
        state = "working";
        detail = "Execution started";
        break;
      case "turn_started":
        state = "working";
        detail = "Agent is working";
        break;
      case "assistant_message":
        state = "working";
        detail = "Response received";
        break;
      case "tool_started":
        toolNames.set(event.kind.call_id, event.kind.name);
        state = "working";
        detail = `Running ${event.kind.name}`;
        break;
      case "tool_finished": {
        const name = toolNames.get(event.kind.call_id) ?? "tool";
        state = event.kind.is_error ? "failed" : "working";
        detail = event.kind.is_error ? `${name} failed` : `Finished ${name}`;
        break;
      }
      case "execution_terminated":
        if (event.kind.terminal.status === "completed") {
          state = "ready";
          detail = "Ready to continue";
        } else if (event.kind.terminal.status === "failed") {
          state = "failed";
          detail = event.kind.terminal.error;
        } else {
          state = "cancelled";
          detail = event.kind.terminal.reason;
        }
        break;
    }
  }

  if (!attached) {
    ({ state, detail } = DIRECTORY_VIEW[task.status]);
  }

  return {
    taskId: task.taskId,
    target: task.target,
    title: taskTitle(task.target),
    state,
    detail,
    lastRecordedAtMs,
    eventCount: events.length,
  };
}

export function taskTitle(target: string): string {
  const withoutKind = target.includes(":") ? target.slice(target.indexOf(":") + 1) : target;
  const normalized = withoutKind.replace(/\\/g, "/").replace(/\/$/, "");
  const finalSegment = normalized.split("/").filter(Boolean).at(-1) ?? target;
  const title = finalSegment
    .replace(/[-_]+/g, " ")
    .replace(/\b\w/g, (letter) => letter.toUpperCase());
  return title.replace(/\b(Api|Acp|Github|Mcp|Rcp|Vps)\b/g, (word) => TITLE_WORDS[word] ?? word);
}

export function taskKind(target: string): string {
  const separator = target.indexOf(":");
  return separator === -1 ? "Task" : `${taskTitle(target.slice(0, separator))} task`;
}

export function latestTaskEvent(events: readonly TaskEvent[]): TaskEvent | undefined {
  return events.at(-1);
}

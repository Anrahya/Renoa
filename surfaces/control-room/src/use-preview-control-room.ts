import { useState } from "react";
import type { TaskEvent, TaskSummary } from "@renoa/rcp-client/browser";

import type { ControlRoomController } from "./use-control-room";

const TASKS: readonly TaskSummary[] = [
  task(10, "workspace:renoa"),
  task(11, "workspace:waku"),
  task(12, "telegram:arcee"),
  task(13, "service:github-review"),
  task(14, "workspace:integrations"),
  task(15, "service:vps-operator"),
  task(16, "service:news-digest"),
];

const INITIAL_EVENTS: Readonly<Record<string, readonly TaskEvent[]>> = {
  [TASKS[0]!.taskId]: [command(0, TASKS[0]!, "Refine the Control Room"), started(1, TASKS[0]!), tool(2, TASKS[0]!)],
  [TASKS[1]!.taskId]: [command(0, TASKS[1]!, "Update the Waku integration"), completed(1, TASKS[1]!)],
  [TASKS[2]!.taskId]: [command(0, TASKS[2]!, "Check the Telegram service"), completed(1, TASKS[2]!)],
};

export function usePreviewControlRoom(): ControlRoomController {
  const [selectedTaskId, setSelectedTaskId] = useState<string | null>(TASKS[0]!.taskId);
  const [events, setEvents] = useState(INITIAL_EVENTS);

  async function submit(text: string): Promise<void> {
    if (selectedTaskId === null) return;
    const taskSummary = TASKS.find((taskItem) => taskItem.taskId === selectedTaskId);
    if (taskSummary === undefined) return;
    setEvents((current) => {
      const journal = current[selectedTaskId] ?? [];
      return { ...current, [selectedTaskId]: [...journal, command(journal.length, taskSummary, text)] };
    });
  }

  return {
    connection: "connected",
    error: null,
    tasks: TASKS,
    events,
    selectedTaskId,
    pendingCount: 0,
    busy: false,
    savedPrincipalId: "00000000-0000-0000-0000-000000000001",
    unlock: async () => undefined,
    register: async () => undefined,
    reconnect: async () => undefined,
    leave: async () => undefined,
    refresh: async () => undefined,
    selectTask: async (taskId) => setSelectedTaskId(taskId),
    submit,
    retryPending: async () => undefined,
  };
}

function task(suffix: number, target: string): TaskSummary {
  return { taskId: id(suffix), target };
}

function command(sequence: number, taskSummary: TaskSummary, text: string): TaskEvent {
  return {
    eventId: id(100 + Number(taskSummary.taskId.slice(-2)) * 10 + sequence),
    taskId: taskSummary.taskId,
    sequence,
    kind: {
      type: "command_submitted",
      command: {
        commandId: id(200 + Number(taskSummary.taskId.slice(-2))),
        principalId: id(1),
        surface: "control_room",
        target: taskSummary.target,
        input: { type: "text", text },
      },
    },
  };
}

function started(sequence: number, taskSummary: TaskSummary): TaskEvent {
  return execution(sequence, taskSummary, { type: "turn_started" });
}

function tool(sequence: number, taskSummary: TaskSummary): TaskEvent {
  return execution(sequence, taskSummary, {
    type: "tool_started",
    call_id: "call_01",
    name: "read_file",
    arguments: { path: "surfaces/control-room/src/workspace.tsx" },
  });
}

function completed(sequence: number, taskSummary: TaskSummary): TaskEvent {
  return execution(sequence, taskSummary, {
    type: "execution_terminated",
    terminal: { status: "completed" },
  });
}

function execution(
  sequence: number,
  taskSummary: TaskSummary,
  kind: Extract<TaskEvent["kind"], { type: "execution_event" }>["event"]["kind"],
): TaskEvent {
  const taskNumber = Number(taskSummary.taskId.slice(-2));
  return {
    eventId: id(300 + taskNumber * 10 + sequence),
    taskId: taskSummary.taskId,
    sequence,
    kind: {
      type: "execution_event",
      commandId: id(200 + taskNumber),
      event: {
        eventId: id(400 + taskNumber * 10 + sequence),
        executionId: id(500 + taskNumber),
        sequence,
        recordedAtMs: 1_788_278_400_000 + sequence * 1_000,
        kind,
      },
    },
  };
}

function id(value: number): string {
  return `00000000-0000-0000-0000-${String(value).padStart(12, "0")}`;
}

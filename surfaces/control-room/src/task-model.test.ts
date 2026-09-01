import { describe, expect, test } from "vitest";
import type { TaskEvent, TaskSummary } from "@renoa/rcp-client/browser";

import { projectTask, taskTitle } from "./task-model";

describe("task projection", () => {
  test("reports only states proved by durable events", () => {
    expect(projectTask(TASK, []).state).toBe("ready");
    expect(projectTask(TASK, [command(0)]).state).toBe("queued");
    expect(projectTask(TASK, [command(0), execution(1, { type: "turn_started" })]).state)
      .toBe("working");
    expect(
      projectTask(TASK, [
        command(0),
        execution(1, { type: "execution_terminated", terminal: { status: "completed" } }),
      ]),
    ).toMatchObject({ state: "ready", detail: "Ready to continue" });
  });

  test("uses a readable title without inventing task metadata", () => {
    expect(taskTitle("workspace:/home/anrahya/Code/personal/Renoa")).toBe("Renoa");
    expect(taskTitle("project:github-review-agent")).toBe("GitHub Review Agent");
    expect(taskTitle("service:vps-operator")).toBe("VPS Operator");
  });
});

const TASK: TaskSummary = {
  taskId: "00000000-0000-0000-0000-000000000010",
  target: "workspace:renoa",
};

function command(sequence: number): TaskEvent {
  return {
    eventId: `00000000-0000-0000-0000-${String(30 + sequence).padStart(12, "0")}`,
    taskId: TASK.taskId,
    sequence,
    kind: {
      type: "command_submitted",
      command: {
        commandId: "00000000-0000-0000-0000-000000000020",
        principalId: "00000000-0000-0000-0000-000000000040",
        surface: "control_room",
        target: TASK.target,
        input: { type: "text", text: "continue" },
      },
    },
  };
}

function execution(
  sequence: number,
  kind: Extract<TaskEvent["kind"], { type: "execution_event" }>["event"]["kind"],
): TaskEvent {
  return {
    eventId: `00000000-0000-0000-0000-${String(30 + sequence).padStart(12, "0")}`,
    taskId: TASK.taskId,
    sequence,
    kind: {
      type: "execution_event",
      commandId: "00000000-0000-0000-0000-000000000020",
      event: {
        eventId: `00000000-0000-0000-0000-${String(50 + sequence).padStart(12, "0")}`,
        executionId: "00000000-0000-0000-0000-000000000060",
        sequence,
        recordedAtMs: 1_788_278_400_000 + sequence,
        kind,
      },
    },
  };
}

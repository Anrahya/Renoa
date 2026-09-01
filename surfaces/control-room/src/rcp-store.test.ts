import "fake-indexeddb/auto";
import { describe, expect, test } from "vitest";
import type { TaskEvent } from "@renoa/rcp-client/browser";

import { ControlRoomStore } from "./rcp-store";

describe("ControlRoomStore", () => {
  test("keeps cursors monotonic and commands in durable insertion order", async () => {
    const store = new ControlRoomStore(databaseName());
    await store.advanceCursor(TASK_ID, 4);
    await store.advanceCursor(TASK_ID, 2);
    expect(await store.cursor(TASK_ID)).toBe(4);

    await store.enqueueCommand({ commandId: COMMAND_A, taskId: TASK_ID, text: "first" });
    await store.enqueueCommand({ commandId: COMMAND_B, taskId: TASK_ID, text: "second" });
    expect(await store.pendingCommands()).toEqual([
      { commandId: COMMAND_A, taskId: TASK_ID, text: "first" },
      { commandId: COMMAND_B, taskId: TASK_ID, text: "second" },
    ]);
    await store.removeCommand(COMMAND_A);
    expect(await store.pendingCommands()).toEqual([
      { commandId: COMMAND_B, taskId: TASK_ID, text: "second" },
    ]);
    await store.close();
  });

  test("replay is idempotent but cannot replace durable event identity", async () => {
    const store = new ControlRoomStore(databaseName());
    const event = taskEvent(EVENT_A, 0, "hello");
    await store.persistEvent(event);
    await store.persistEvent(event);
    expect(await store.eventsForTask(TASK_ID)).toEqual([event]);

    await expect(
      store.persistEvent(taskEvent(EVENT_A, 0, "changed")),
    ).rejects.toThrow("changed during replay");
    await expect(
      store.persistEvent(taskEvent(EVENT_B, 0, "different identity")),
    ).rejects.toThrow();
    expect(await store.eventsForTask(TASK_ID)).toEqual([event]);
    await store.close();
  });
});

const TASK_ID = "00000000-0000-0000-0000-000000000010";
const COMMAND_A = "00000000-0000-0000-0000-000000000020";
const COMMAND_B = "00000000-0000-0000-0000-000000000021";
const EVENT_A = "00000000-0000-0000-0000-000000000030";
const EVENT_B = "00000000-0000-0000-0000-000000000031";

function taskEvent(eventId: string, sequence: number, text: string): TaskEvent {
  return {
    eventId,
    taskId: TASK_ID,
    sequence,
    kind: {
      type: "command_submitted",
      command: {
        commandId: COMMAND_A,
        principalId: "00000000-0000-0000-0000-000000000040",
        surface: "control_room",
        target: "workspace:renoa",
        input: { type: "text", text },
      },
    },
  };
}

function databaseName(): string {
  return `renoa-control-room-test-${crypto.randomUUID()}`;
}

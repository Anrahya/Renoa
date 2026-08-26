import assert from "node:assert/strict";
import { test } from "node:test";

import { parseServerMessage } from "../src/protocol.js";

const TASK_ID = "00000000-0000-0000-0000-000000000001";
const TASK_EVENT_ID = "00000000-0000-0000-0000-000000000002";
const COMMAND_ID = "00000000-0000-0000-0000-000000000003";
const EXECUTION_EVENT_ID = "00000000-0000-0000-0000-000000000004";
const EXECUTION_ID = "00000000-0000-0000-0000-000000000005";

test("execution task records preserve stable command causation", () => {
  const message = parseServerMessage(
    JSON.stringify({
      type: "task_event",
      event: {
        eventId: TASK_EVENT_ID,
        taskId: TASK_ID,
        sequence: 9,
        kind: {
          type: "execution_event",
          commandId: COMMAND_ID,
          event: {
            eventId: EXECUTION_EVENT_ID,
            executionId: EXECUTION_ID,
            sequence: 0,
            recordedAtMs: 10,
            kind: { type: "execution_started" },
          },
        },
      },
    }),
  );

  assert.equal(message.type, "task_event");
  assert.equal(message.event.kind.type, "execution_event");
  assert.equal(message.event.kind.commandId, COMMAND_ID);
  assert.equal(message.event.kind.event.executionId, EXECUTION_ID);
});

test("execution task records without command causation are rejected", () => {
  assert.throws(
    () =>
      parseServerMessage(
        JSON.stringify({
          type: "task_event",
          event: {
            eventId: TASK_EVENT_ID,
            taskId: TASK_ID,
            sequence: 9,
            kind: {
              type: "execution_event",
              event: {
                eventId: EXECUTION_EVENT_ID,
                executionId: EXECUTION_ID,
                sequence: 0,
                recordedAtMs: 10,
                kind: { type: "execution_started" },
              },
            },
          },
        }),
      ),
    /execution event commandId must be a string/,
  );
});

test("the baseline execution activity profile is decoded without raw records", () => {
  const kinds = [
    { type: "turn_started" },
    { type: "assistant_message", text: "done" },
    { type: "tool_started", call_id: "call-1", name: "read", arguments: { path: "a" } },
    { type: "tool_finished", call_id: "call-1", output: "contents", is_error: false },
    { type: "execution_terminated", terminal: { status: "failed", error: "stopped" } },
  ] as const;

  for (const kind of kinds) {
    const message = parseServerMessage(executionTaskEvent(kind));
    assert.equal(message.type, "task_event");
    assert.equal(message.event.kind.type, "execution_event");
    assert.deepEqual(message.event.kind.event.kind, kind);
  }
});

test("server numbers outside JavaScript's exact range are rejected", () => {
  assert.throws(
    () =>
      parseServerMessage(
        JSON.stringify({
          type: "task_list",
          request_id: Number.MAX_SAFE_INTEGER + 1,
          tasks: [],
        }),
      ),
    /request_id must be a safe unsigned integer/,
  );
});

function executionTaskEvent(kind: unknown): string {
  return JSON.stringify({
    type: "task_event",
    event: {
      eventId: TASK_EVENT_ID,
      taskId: TASK_ID,
      sequence: 9,
      kind: {
        type: "execution_event",
        commandId: COMMAND_ID,
        event: {
          eventId: EXECUTION_EVENT_ID,
          executionId: EXECUTION_ID,
          sequence: 1,
          recordedAtMs: 10,
          kind,
        },
      },
    },
  });
}

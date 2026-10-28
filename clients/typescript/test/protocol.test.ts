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

test("submitted commands keep the surface's context when it has one", () => {
  const submitted = (input: unknown) => {
    const message = parseServerMessage(
      JSON.stringify({
        type: "task_event",
        event: {
          eventId: TASK_EVENT_ID,
          taskId: TASK_ID,
          sequence: 9,
          kind: {
            type: "command_submitted",
            command: {
              commandId: COMMAND_ID,
              principalId: EXECUTION_ID,
              surface: "discord",
              target: "agent:x",
              input,
            },
          },
        },
      }),
    );
    assert.equal(message.type, "task_event");
    assert.equal(message.event.kind.type, "command_submitted");
    return message.event.kind.command.input;
  };

  assert.deepEqual(submitted({ type: "text", text: "hi" }), { type: "text", text: "hi" });
  assert.deepEqual(submitted({ type: "text", text: "hi", context: "channel #general" }), {
    type: "text",
    text: "hi",
    context: "channel #general",
  });
  assert.throws(() => submitted({ type: "text", text: "hi", context: 1 }), /command context/);
  assert.deepEqual(submitted({ type: "text", text: "hi", author: "guest" }), {
    type: "text",
    text: "hi",
    author: "guest",
  });
  assert.throws(
    () => submitted({ type: "text", text: "hi", author: "principal" }),
    /command author/,
  );
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

test("task summaries carry a status and reject one the client does not know", () => {
  const list = (status: unknown) =>
    parseServerMessage(
      JSON.stringify({
        type: "task_list",
        request_id: 4,
        tasks: [{ taskId: TASK_ID, target: "workspace:renoa", status }],
      }),
    );
  for (const status of ["working", "idle", "failed"] as const) {
    const message = list(status);
    assert.equal(message.type, "task_list");
    assert.equal(message.tasks[0]?.status, status);
  }
  assert.throws(() => list("waiting"), /unsupported task status waiting/);
  assert.throws(() => list(undefined), /task status must be a string/);
});

test("a stop answer names the command and one of two outcomes", () => {
  const accepted = (outcome: unknown) =>
    parseServerMessage(
      JSON.stringify({
        type: "stop_accepted",
        request_id: 5,
        command_id: COMMAND_ID,
        outcome,
      }),
    );
  assert.deepEqual(accepted("requested"), {
    type: "stop_accepted",
    request_id: 5,
    command_id: COMMAND_ID,
    outcome: "requested",
  });
  assert.equal(accepted("already_ended").type, "stop_accepted");
  assert.throws(() => accepted("stopped"), /unsupported stop outcome stopped/);
});

test("live activity is a numbered text delta or a reset and carries no task sequence", () => {
  const live = (activity: unknown) =>
    parseServerMessage(
      JSON.stringify({
        type: "live_activity",
        task_id: TASK_ID,
        command_id: COMMAND_ID,
        activity,
      }),
    );
  assert.deepEqual(live({ type: "text_delta", sequence: 3, text: "Hel" }), {
    type: "live_activity",
    task_id: TASK_ID,
    command_id: COMMAND_ID,
    activity: { type: "text_delta", sequence: 3, text: "Hel" },
  });
  assert.deepEqual(live({ type: "text_reset" }).type, "live_activity");
  assert.throws(() => live({ type: "reasoning", text: "x" }), /unsupported live activity/);
  assert.throws(
    () => live({ type: "text_delta", sequence: Number.MAX_SAFE_INTEGER + 1, text: "x" }),
    /live text sequence must be a safe unsigned integer/,
  );
});

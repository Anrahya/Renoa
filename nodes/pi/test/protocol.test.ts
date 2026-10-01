import assert from "node:assert/strict";
import { test } from "node:test";

import { parseServerMessage } from "../src/protocol.js";

test("an RCP execute message contains only harness-neutral command data", () => {
  assert.deepEqual(
    parseServerMessage(
      JSON.stringify({
        type: "execute",
        task_id: "00000000-0000-0000-0000-000000000001",
        command: {
          commandId: "00000000-0000-0000-0000-000000000002",
          principalId: "00000000-0000-0000-0000-000000000004",
          surface: "phone",
          target: "workspace:renoa",
          input: { type: "text", text: "continue" },
        },
      }),
    ),
    {
      type: "execute",
      command: {
        taskId: "00000000-0000-0000-0000-000000000001",
        commandId: "00000000-0000-0000-0000-000000000002",
        principalId: "00000000-0000-0000-0000-000000000004",
        surface: "phone",
        target: "workspace:renoa",
        text: "continue",
      },
    },
  );
});

test("an execute message marks a guest's command and rejects an unknown author", () => {
  const execute = (author: unknown) =>
    parseServerMessage(
      JSON.stringify({
        type: "execute",
        task_id: "00000000-0000-0000-0000-000000000001",
        command: {
          commandId: "00000000-0000-0000-0000-000000000002",
          principalId: "00000000-0000-0000-0000-000000000004",
          surface: "discord",
          target: "workspace:renoa",
          input: { type: "text", text: "hi", author },
        },
      }),
    );
  const parsed = execute("guest");
  assert.equal(parsed.type, "execute");
  assert.equal(parsed.command.guest, true);
  assert.throws(() => execute("principal"), /command author/);
});

test("RCP numbers outside JavaScript's exact range are rejected", () => {
  assert.throws(
    () =>
      parseServerMessage(
        JSON.stringify({
          type: "execution_events_accepted",
          command_id: "00000000-0000-0000-0000-000000000001",
          through_execution_sequence: Number.MAX_SAFE_INTEGER + 1,
        }),
      ),
    /through_execution_sequence must be a safe unsigned integer/,
  );
});

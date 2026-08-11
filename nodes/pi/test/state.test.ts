import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { after, before, test } from "node:test";

import { NodeState } from "../src/state.js";
import type { ExecuteCommand, ExecutionEventKind } from "../src/protocol.js";

let directory: string;

before(async () => {
  directory = await mkdtemp(join(tmpdir(), "renoa-pi-state-"));
});

after(async () => {
  await rm(directory, { force: true, recursive: true });
});

test("an admitted command survives restart and exact redelivery reuses its identity", () => {
  const path = join(directory, "node.sqlite");
  const command = executeCommand("continue here");
  const first = new NodeState(path);
  const admitted = first.admit(command);
  first.close();

  const reopened = new NodeState(path);
  const redelivered = reopened.admit(command);
  assert.equal(redelivered.executionId, admitted.executionId);
  assert.equal(redelivered.admitted, false);
  assert.deepEqual(reopened.nextQueued(), {
    ...command,
    executionId: admitted.executionId,
  });
  reopened.close();
});

test("legacy RCP harness fields are removed without changing command identity", () => {
  const path = join(directory, "harness-fields.sqlite");
  const command = executeCommand("continue here");
  const first = new NodeState(path);
  const admission = first.admit(command);
  first.close();
  const database = new DatabaseSync(path);
  database
    .prepare("UPDATE executions SET command_json = ? WHERE command_id = ?")
    .run(
      JSON.stringify({
        ...command,
        agentId: "00000000-0000-0000-0000-000000000003",
        instructions: "Legacy Pi instructions.",
        capabilityGrants: ["workspace.read"],
      }),
      command.commandId,
    );
  database.exec("PRAGMA user_version = 1");
  database.close();

  const migrated = new NodeState(path);
  assert.deepEqual(migrated.admit(command), {
    executionId: admission.executionId,
    admitted: false,
  });
  assert.deepEqual(migrated.nextQueued(), { ...command, executionId: admission.executionId });
  migrated.close();
});

test("reusing a command identity with different content is rejected", () => {
  const state = new NodeState(join(directory, "conflict.sqlite"));
  const command = executeCommand("original");
  state.admit(command);
  assert.throws(
    () => state.admit({ ...command, text: "changed" }),
    /does not match its durable admission/,
  );
  state.close();
});

test("an empty queue does not report a durable change", () => {
  let commits = 0;
  const state = new NodeState(join(directory, "empty.sqlite"), () => {
    commits += 1;
  });

  assert.equal(state.claimNext(), null);
  assert.equal(commits, 0);
  state.close();
});

test("execution activity remains pending until the coordinator accepts its cursor", () => {
  const state = new NodeState(join(directory, "events.sqlite"));
  const command = executeCommand("publish me");
  state.admit(command);
  const running = state.claimNext();
  assert.equal(running?.commandId, command.commandId);
  state.appendEvent(command.commandId, { type: "turn_started" });

  const pending = state.pendingPublications();
  assert.equal(pending.length, 1);
  assert.equal(pending[0]?.admissionAcked, false);
  assert.deepEqual(
    pending[0]?.events.map((event) => event.kind),
    [{ type: "execution_started" }, { type: "turn_started" }],
  );

  state.acknowledgeAdmission(command.commandId);
  state.advancePublication(command.commandId, 0);
  assert.deepEqual(
    state.pendingPublications()[0]?.events.map((event) => event.kind),
    [{ type: "turn_started" }],
  );
  state.close();
});

test("restart fails an interrupted execution without running it twice", () => {
  const path = join(directory, "recovery.sqlite");
  const command = executeCommand("do this once");
  const first = new NodeState(path);
  first.admit(command);
  first.claimNext();
  first.close();

  const reopened = new NodeState(path);
  reopened.recoverInterrupted();
  assert.equal(reopened.nextQueued(), null);
  const kinds = reopened.pendingPublications()[0]?.events.map((event) => event.kind);
  assert.deepEqual(kinds, [
    { type: "execution_started" },
    {
      type: "execution_terminated",
      terminal: { status: "failed", error: "execution interrupted by node restart" },
    },
  ] satisfies ExecutionEventKind[]);
  reopened.close();
});

test("a database from a newer node version is rejected", () => {
  const path = join(directory, "newer.sqlite");
  const database = new DatabaseSync(path);
  database.exec("PRAGMA user_version = 3");
  database.close();

  assert.throws(
    () => new NodeState(path),
    /schema 3 is newer than supported version 2/,
  );
});

function executeCommand(text: string): ExecuteCommand {
  return {
    taskId: "00000000-0000-0000-0000-000000000001",
    commandId: "00000000-0000-0000-0000-000000000002",
    principalId: "00000000-0000-0000-0000-000000000004",
    surface: "test",
    target: "workspace:renoa",
    text,
  };
}

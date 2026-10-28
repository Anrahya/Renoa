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

test("a stop is recorded once, even before its command is admitted", () => {
  const state = new NodeState(join(directory, "stop-before.sqlite"));
  const command = executeCommand("continue here");

  assert.equal(state.recordStop(command.taskId, command.commandId), "recorded");
  assert.equal(state.recordStop(command.taskId, command.commandId), "recorded");
  const watch = state.watchStop(command.commandId);
  assert.equal(watch.signal.aborted, true, "a command admitted after its stop starts stopped");
  assert.equal(state.watchStop("00000000-0000-0000-0000-000000000009").signal.aborted, false);
  state.close();
});

test("a stop reaches a running command and no other, until it is released", () => {
  const state = new NodeState(join(directory, "stop-running.sqlite"));
  const command = executeCommand("continue here");
  state.admit(command);
  state.claimNext();
  const watch = state.watchStop(command.commandId);
  const other = state.watchStop("00000000-0000-0000-0000-000000000009");
  assert.equal(watch.signal.aborted, false);

  state.recordStop(command.taskId, command.commandId);

  assert.equal(watch.signal.aborted, true);
  assert.equal(other.signal.aborted, false);
  watch.release();
  other.release();
  state.close();
});

test("a released watch is no longer aborted by a stop", () => {
  const state = new NodeState(join(directory, "stop-released.sqlite"));
  const command = executeCommand("continue here");
  state.admit(command);
  const watch = state.watchStop(command.commandId);
  watch.release();

  state.recordStop(command.taskId, command.commandId);

  assert.equal(watch.signal.aborted, false);
  state.close();
});

test("a stop for an ended command records nothing and another task's stop is refused", () => {
  const path = join(directory, "stop-ended.sqlite");
  const state = new NodeState(path);
  const command = executeCommand("continue here");
  state.admit(command);
  state.claimNext();
  state.finish(command.commandId, { status: "completed" }, []);

  assert.equal(state.recordStop(command.taskId, command.commandId), "ended");
  assert.equal(state.watchStop(command.commandId).signal.aborted, false);
  assert.throws(
    () => state.recordStop("00000000-0000-0000-0000-000000000008", command.commandId),
    /does not belong to task/,
  );
  state.close();
  const database = new DatabaseSync(path, { readOnly: true });
  const stops = database.prepare("SELECT count(*) AS count FROM stops").get() as {
    readonly count: number;
  };
  database.close();
  assert.equal(stops.count, 0);
});

test("a stop survives a restart", () => {
  const path = join(directory, "stop-restart.sqlite");
  const command = executeCommand("continue here");
  const first = new NodeState(path);
  first.recordStop(command.taskId, command.commandId);
  first.close();

  const reopened = new NodeState(path);

  assert.equal(reopened.watchStop(command.commandId).signal.aborted, true);
  reopened.close();
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

import assert from "node:assert/strict";
import { after, before, test } from "node:test";

import { RcpError, RcpSurfaceClient, type TaskEvent } from "../src/index.js";
import {
  RcpSurfaceClientCore,
  type PendingTextCommand,
  type RcpSurfaceState,
} from "../src/core-client.js";
import { SurfaceState } from "../src/state.js";
import { type Fixture, startFixture } from "./fixture.js";

let fixture: Fixture;

before(async () => {
  fixture = await startFixture();
});

after(async () => {
  await fixture.stop();
});

test("a command survives a lost acknowledgement without duplicate admission", async () => {
  const first = client();
  await first.connect();
  await first.attach(fixture.tasks[0].taskId, () => {});

  const submission = await first.submitText(
    fixture.tasks[0].taskId,
    "continue after reconnect",
  );
  await assert.rejects(submission.accepted);
  await first.close();

  const replayed: TaskEvent[] = [];
  const resumed = client();
  await resumed.connect();
  await resumed.attach(fixture.tasks[0].taskId, (event) => {
    replayed.push(event);
  });
  assert.equal(
    replayed.filter((event) => commandId(event) === submission.commandId).length,
    1,
  );
  assert.deepEqual(await resumed.retryPendingCommands(), [submission.commandId]);
  assert.deepEqual(await resumed.retryPendingCommands(), []);
  await resumed.close();
});

test("a command for an offline node remains durable across process restart", async () => {
  const statePath = `${fixture.statePath}.offline-node`;
  const first = new RcpSurfaceClient({
    endpoint: fixture.endpoint,
    authentication: { type: "device", credentials: fixture.credentials },
    statePath,
  });
  await first.connect();

  const submission = await first.submitText(
    fixture.tasks[2].taskId,
    "continue when the node returns",
  );
  await assert.rejects(submission.accepted, isNodeOffline);
  assert.deepEqual(await first.listTasks(), fixture.tasks);
  await first.close();

  const resumed = new RcpSurfaceClient({
    endpoint: fixture.endpoint,
    authentication: { type: "device", credentials: fixture.credentials },
    statePath,
  });
  await resumed.connect();
  await assert.rejects(resumed.retryPendingCommands(), isNodeOffline);
  await resumed.close();
});

test("an asynchronous browser-style outbox commits before transmission", { timeout: 5_000 }, async () => {
  const state = new GatedAsyncState(`${fixture.statePath}.async-state`);
  const client = new RcpSurfaceClientCore({
    endpoint: fixture.endpoint,
    authentication: { type: "device", credentials: fixture.credentials },
    state,
  });
  let observed = false;
  const delivered = Promise.withResolvers<void>();
  await client.connect();
  await client.attach(fixture.tasks[0].taskId, (event) => {
    if (
      event.kind.type === "command_submitted" &&
      event.kind.command.input.text === "persist before send"
    ) {
      observed = true;
      delivered.resolve();
    }
  });

  const submitting = client.submitText(fixture.tasks[0].taskId, "persist before send");
  await state.enqueueStarted.promise;
  assert.equal(observed, false);
  state.allowEnqueue.resolve();
  const submission = await submitting;
  await submission.accepted;
  await delivered.promise;
  assert.equal(observed, true);
  await client.close();
});

function client(): RcpSurfaceClient {
  return new RcpSurfaceClient({
    endpoint: fixture.lossyEndpoint,
    authentication: { type: "device", credentials: fixture.credentials },
    statePath: fixture.statePath,
  });
}

function commandId(event: TaskEvent): unknown {
  return event.kind.type === "command_submitted" ? event.kind.command.commandId : undefined;
}

function isNodeOffline(error: unknown): boolean {
  assert.ok(error instanceof RcpError);
  assert.equal(error.code, "node_offline");
  assert.notEqual(error.requestId, null);
  return true;
}

class GatedAsyncState implements RcpSurfaceState {
  readonly enqueueStarted = Promise.withResolvers<void>();
  readonly allowEnqueue = Promise.withResolvers<void>();
  readonly #state: SurfaceState;

  constructor(path: string) {
    this.#state = new SurfaceState(path);
  }

  async cursor(taskId: string): Promise<number | null> {
    return this.#state.cursor(taskId);
  }

  async advanceCursor(taskId: string, sequence: number): Promise<void> {
    this.#state.advanceCursor(taskId, sequence);
  }

  async enqueueCommand(command: PendingTextCommand): Promise<void> {
    this.enqueueStarted.resolve();
    await this.allowEnqueue.promise;
    this.#state.enqueueCommand(command);
  }

  async pendingCommands(): Promise<readonly PendingTextCommand[]> {
    return this.#state.pendingCommands();
  }

  async removeCommand(commandId: string): Promise<void> {
    this.#state.removeCommand(commandId);
  }

  async close(): Promise<void> {
    this.#state.close();
  }
}

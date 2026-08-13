import assert from "node:assert/strict";
import { after, before, test } from "node:test";

import { RcpError, RcpSurfaceClient, type TaskEvent } from "../src/index.js";
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

  const submission = first.submitText(fixture.tasks[0].taskId, "continue after reconnect");
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
    credentials: fixture.credentials,
    statePath,
  });
  await first.connect();

  const submission = first.submitText(fixture.tasks[2].taskId, "continue when the node returns");
  await assert.rejects(submission.accepted, isNodeOffline);
  assert.deepEqual(await first.listTasks(), fixture.tasks);
  await first.close();

  const resumed = new RcpSurfaceClient({
    endpoint: fixture.endpoint,
    credentials: fixture.credentials,
    statePath,
  });
  await resumed.connect();
  await assert.rejects(resumed.retryPendingCommands(), isNodeOffline);
  await resumed.close();
});

function client(): RcpSurfaceClient {
  return new RcpSurfaceClient({
    endpoint: fixture.lossyEndpoint,
    credentials: fixture.credentials,
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

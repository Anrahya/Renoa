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

test("a surface resumes after its last durably applied event", async () => {
  const replayed: TaskEvent[] = [];
  const first = client();
  await first.connect();
  await first.attach(fixture.tasks[0].taskId, (event) => {
    replayed.push(event);
  });
  assert.deepEqual(
    replayed.map((event) => event.sequence),
    [0],
  );
  await first.close();

  const duplicates: TaskEvent[] = [];
  const resumed = client();
  await resumed.connect();
  await resumed.attach(fixture.tasks[0].taskId, (event) => {
    duplicates.push(event);
  });
  assert.deepEqual(duplicates, []);
  await resumed.close();
});

test("reconnecting restores live delivery for existing attachments", async () => {
  let expectedCommandId: string | undefined;
  const observed = Promise.withResolvers<void>();
  const surface = client();
  await surface.connect();
  await surface.attach(fixture.tasks[0].taskId, (event) => {
    if (commandId(event) === expectedCommandId) {
      observed.resolve();
    }
  });

  await surface.disconnect();
  await surface.connect();
  const submission = surface.submitText(fixture.tasks[0].taskId, "continue on this socket");
  expectedCommandId = submission.commandId;
  await Promise.all([submission.accepted, observed.promise]);
  await surface.close();
});

test("a failed event projection is not checkpointed", async () => {
  const statePath = `${fixture.statePath}.failed-projection`;
  const seed = client(`${statePath}.seed`);
  await seed.connect();
  await seed.submitText(fixture.tasks[1].taskId, "create a projection test event").accepted;
  await seed.close();

  const first = client(statePath);
  await first.connect();
  await assert.rejects(
    first.attach(fixture.tasks[1].taskId, () => {
      throw new Error("projection failed");
    }),
    /projection failed/,
  );
  await first.close();

  const replayed: number[] = [];
  const resumed = client(statePath);
  await resumed.connect();
  await resumed.attach(fixture.tasks[1].taskId, (event) => {
    replayed.push(event.sequence);
  });
  assert.deepEqual(replayed, [0]);
  await resumed.close();
});

test("a replay-required disconnect is observable and recoverable", async () => {
  const observed = Promise.withResolvers<void>();
  let expectedCommandId: string | undefined;
  const surface = new RcpSurfaceClient({
    endpoint: fixture.replayEndpoint,
    credentials: fixture.credentials,
    statePath: `${fixture.statePath}.replay-required`,
  });
  await surface.connect();
  const disconnected = surface.waitForDisconnect();
  const apply = (event: TaskEvent) => {
    if (commandId(event) === expectedCommandId) {
      observed.resolve();
    }
  };
  const attachmentFailed = assert.rejects(
    surface.attach(fixture.tasks[0].taskId, apply),
    isReplayRequired,
  );

  const reason = await disconnected;
  assert.ok(isReplayRequired(reason));
  await attachmentFailed;

  await surface.connect();
  await surface.attach(fixture.tasks[0].taskId, apply);
  const submission = surface.submitText(fixture.tasks[0].taskId, "resume after replay recovery");
  expectedCommandId = submission.commandId;
  await Promise.all([submission.accepted, observed.promise]);
  await surface.close();
});

function client(statePath = fixture.statePath): RcpSurfaceClient {
  return new RcpSurfaceClient({
    endpoint: fixture.endpoint,
    credentials: fixture.credentials,
    statePath,
  });
}

function commandId(event: TaskEvent): unknown {
  const command = event.kind.command;
  return typeof command === "object" && command !== null && "commandId" in command
    ? command.commandId
    : undefined;
}

function isReplayRequired(error: unknown): boolean {
  assert.ok(error instanceof RcpError);
  assert.equal(error.code, "replay_required");
  assert.equal(error.requestId, null);
  return true;
}

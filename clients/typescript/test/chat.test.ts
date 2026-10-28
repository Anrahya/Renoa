import assert from "node:assert/strict";
import { after, before, test } from "node:test";

import {
  type LiveActivity,
  RcpError,
  RcpSurfaceClient,
  type TaskEvent,
} from "../src/index.js";
import { type Fixture, startFixture } from "./fixture.js";

let fixture: Fixture;

before(async () => {
  fixture = await startFixture();
});

after(async () => {
  await fixture.stop();
});

test("a surface that asks for live activity receives the node's frames in order", async () => {
  const frames: Array<{ readonly commandId: string; readonly activity: LiveActivity }> = [];
  const done = Promise.withResolvers<void>();
  const surface = client("live");
  await surface.connect();
  await surface.attach(
    fixture.tasks[1].taskId,
    () => {},
    (commandId, activity) => {
      frames.push({ commandId, activity });
      if (activity.type === "text_reset") {
        done.resolve();
      }
    },
  );

  const submission = await surface.submitText(fixture.tasks[1].taskId, "write something");
  await Promise.all([submission.accepted, done.promise]);

  assert.deepEqual(
    frames.map((frame) => frame.activity),
    [
      { type: "text_delta", sequence: 0, text: "Hel" },
      { type: "text_delta", sequence: 1, text: "lo" },
      { type: "text_reset" },
    ],
  );
  assert.ok(frames.every((frame) => frame.commandId === submission.commandId));
  await surface.close();
});

test("a reconnecting surface asks for live activity again", async () => {
  const done = Promise.withResolvers<void>();
  const received: LiveActivity["type"][] = [];
  const surface = client("reconnect");
  await surface.connect();
  await surface.attach(
    fixture.tasks[1].taskId,
    () => {},
    (_commandId, activity) => {
      received.push(activity.type);
      if (activity.type === "text_reset") {
        done.resolve();
      }
    },
  );

  await surface.disconnect();
  await surface.connect();
  const submission = await surface.submitText(fixture.tasks[1].taskId, "write after reconnecting");
  await Promise.all([submission.accepted, done.promise]);

  assert.deepEqual(received, ["text_delta", "text_delta", "text_reset"]);
  await surface.close();
});

test("a surface that did not ask for live activity receives none", async () => {
  const events: TaskEvent[] = [];
  const observed = Promise.withResolvers<void>();
  const surface = client("plain");
  await surface.connect();
  // A frame for an attachment that did not ask would disconnect the client.
  const disconnected = surface.waitForDisconnect();
  let commandId: string | undefined;
  await surface.attach(fixture.tasks[1].taskId, (event) => {
    events.push(event);
    if (
      event.kind.type === "command_submitted" &&
      event.kind.command.commandId === commandId
    ) {
      observed.resolve();
    }
  });

  const submission = await surface.submitText(fixture.tasks[1].taskId, "write quietly");
  commandId = submission.commandId;
  await Promise.all([submission.accepted, observed.promise]);
  const settled = await Promise.race([
    disconnected,
    new Promise<"connected">((resolve) => setTimeout(() => resolve("connected"), 300)),
  ]);

  assert.equal(settled, "connected");
  await surface.close();
});

test("stopping a command is idempotent and refuses a command the task never admitted", async () => {
  const surface = client("stop");
  await surface.connect();
  const submission = await surface.submitText(fixture.tasks[1].taskId, "run for a while");
  await submission.accepted;

  assert.equal(
    await surface.stopCommand(fixture.tasks[1].taskId, submission.commandId),
    "requested",
  );
  assert.equal(
    await surface.stopCommand(fixture.tasks[1].taskId, submission.commandId),
    "requested",
    "a retry reads the stop that was recorded",
  );
  await assert.rejects(
    surface.stopCommand(fixture.tasks[1].taskId, globalThis.crypto.randomUUID()),
    (error: unknown) => error instanceof RcpError && error.code === "not_found",
  );
  await surface.close();
});

function client(name: string): RcpSurfaceClient {
  return new RcpSurfaceClient({
    endpoint: fixture.endpoint,
    authentication: { type: "device", credentials: fixture.credentials },
    statePath: `${fixture.statePath}.chat-${name}`,
  });
}

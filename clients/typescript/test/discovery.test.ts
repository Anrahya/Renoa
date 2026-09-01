import assert from "node:assert/strict";
import { after, before, test } from "node:test";

import { RcpSurfaceClient } from "../src/index.js";
import { type Fixture, startFixture } from "./fixture.js";

let fixture: Fixture;

before(async () => {
  fixture = await startFixture();
});

after(async () => {
  await fixture.stop();
});

test("an authenticated surface discovers only its tasks", async () => {
  const client = new RcpSurfaceClient({
    endpoint: fixture.endpoint,
    authentication: { type: "device", credentials: fixture.credentials },
    statePath: fixture.statePath,
  });

  await client.connect();
  assert.deepEqual(await client.listTasks(), fixture.tasks);
  await client.close();
});

import assert from "node:assert/strict";
import { after, before, test } from "node:test";

import { RcpError, RcpSurfaceClient } from "../src/index.js";
import { type Fixture, startFixture } from "./fixture.js";

let fixture: Fixture;

before(async () => {
  fixture = await startFixture();
});

after(async () => {
  await fixture.stop();
});

test("authentication failures preserve the RCP error code", async () => {
  const credential = fixture.credentials.credential;
  const client = new RcpSurfaceClient({
    endpoint: fixture.endpoint,
    credentials: {
      ...fixture.credentials,
      credential: `${credential[0] === "0" ? "1" : "0"}${credential.slice(1)}`,
    },
    statePath: `${fixture.statePath}.authentication-failure`,
  });

  await assert.rejects(client.connect(), (error: unknown) => {
    assert.ok(error instanceof RcpError);
    assert.equal(error.code, "authentication_failed");
    assert.equal(error.requestId, null);
    return true;
  });
  await client.close();
});

test("a denied task request fails without closing the connection", async () => {
  const client = new RcpSurfaceClient({
    endpoint: fixture.endpoint,
    credentials: fixture.credentials,
    statePath: `${fixture.statePath}.authorization-failure`,
  });

  await client.connect();
  await assert.rejects(client.attach(fixture.unownedTaskId, () => {}), (error) => {
    assert.ok(error instanceof RcpError);
    assert.equal(error.code, "not_found");
    assert.notEqual(error.requestId, null);
    return true;
  });
  assert.deepEqual(await client.listTasks(), fixture.tasks);
  await client.close();
});

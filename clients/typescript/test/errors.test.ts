import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
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
    authentication: {
      type: "device",
      credentials: {
        ...fixture.credentials,
        credential: `${credential[0] === "0" ? "1" : "0"}${credential.slice(1)}`,
      },
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

test("a browser ticket is requested for the connection and never persisted", async () => {
  const ticket = "ab".repeat(32);
  const statePath = `${fixture.statePath}.ticket-authentication`;
  let requests = 0;
  const client = new RcpSurfaceClient({
    endpoint: fixture.endpoint,
    authentication: {
      type: "ticket",
      getTicket: () => {
        requests += 1;
        return ticket;
      },
    },
    statePath,
  });

  await assert.rejects(client.connect(), (error: unknown) => {
    assert.ok(error instanceof RcpError);
    assert.equal(error.code, "authentication_failed");
    return true;
  });
  assert.equal(requests, 1);
  await client.close();
  const database = await readFile(statePath);
  assert.equal(database.includes(Buffer.from(ticket)), false);
});

test("a denied task request fails without closing the connection", async () => {
  const client = new RcpSurfaceClient({
    endpoint: fixture.endpoint,
    authentication: { type: "device", credentials: fixture.credentials },
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

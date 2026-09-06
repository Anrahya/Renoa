import assert from "node:assert/strict";
import { test } from "node:test";

import { ManualClock, responsesSse, startFakeServer, tempDir } from "./helpers.js";
import { deltas, runStream, streamFailure } from "./stream-support.js";

const rateLimit = {
  status: 429,
  body: JSON.stringify({
    code: "rate_limit_exceeded",
    type: "rate_limit_error",
    message: "Error from provider (Console Go): Rate limit exceeded. Please retry after a brief wait.",
  }),
};

test("OpenCode rate limits recover after longer waits within the same model invocation", async () => {
  const server = await startFakeServer();
  const directory = tempDir();
  const clock = new ManualClock();
  for (let i = 0; i < 4; i += 1) server.enqueue(rateLimit);
  server.enqueue({ sse: responsesSse("recovered") });
  try {
    const records = await runStream({
      directory: directory.path,
      provider: "opencode-go",
      modelId: "grok-4.5",
      baseUrl: server.baseUrl,
      credential: { type: "api_key", key: "fixture-opencode-key" },
      clock,
      releases: 4,
    });
    assert.equal(deltas(records, "text").join(""), "recovered");
    assert.deepEqual(clock.delays, [5_000, 10_000, 20_000, 40_000]);
    assert.equal(server.requests.length, 5);
    // The adapter retries the original request, without synthesizing a new
    // conversation turn or requesting execution of an earlier tool again.
    assert.ok(server.requests.every((request) => request.body === server.requests[0]?.body));
    assert.deepEqual(
      records.filter((record) => record.event === "retry_attempt").map((record) => record.next_attempt),
      [2, 3, 4, 5],
    );
  } finally {
    await server.close();
    directory.close();
  }
});

test("Retry-After is never shortened and cumulative waits remain bounded", async () => {
  for (const scenario of [
    { value: "90", delays: [90_000], requests: 2 },
    { value: new Date(91_000).toUTCString(), delays: [90_000], requests: 2 },
    { value: "3600", delays: [], requests: 1 },
    { value: "70", delays: [70_000], requests: 2 },
  ]) {
    const server = await startFakeServer();
    const directory = tempDir();
    const clock = new ManualClock();
    // On the first response use either seconds or an HTTP-date. Subsequent
    // responses ask for another 70s, which exceeds the remaining wait budget.
    server.enqueue({ ...rateLimit, headers: { "retry-after": scenario.value } });
    server.enqueue({ ...rateLimit, headers: { "retry-after": "70" } });
    try {
      const error = await streamFailure({
        directory: directory.path,
        provider: "opencode-go",
        modelId: "grok-4.5",
        baseUrl: server.baseUrl,
        credential: { type: "api_key", key: "fixture-opencode-key" },
        clock,
        releases: scenario.delays.length,
      });
      assert.equal(error.category, "rate_limited");
      assert.equal(error.attemptCount, scenario.requests);
      assert.equal(server.requests.length, scenario.requests);
      assert.deepEqual(clock.delays, scenario.delays);
      assert.match(error.message, /Try again later or choose another model/);
    } finally {
      await server.close();
      directory.close();
    }
  }
});

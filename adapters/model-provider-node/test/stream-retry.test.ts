import assert from "node:assert/strict";
import { test } from "node:test";

import type { WireStreamRecord } from "../src/contract.js";
import {
  ManualClock,
  jsonError,
  oauthCredential,
  startFakeServer,
  successfulChat,
  tempDir,
} from "./helpers.js";
import {
  deltas,
  runStream,
  streamFailure,
  waitFor,
} from "./stream-support.js";

test("400 with a provider error body is not retried", async () => {
  const server = await startFakeServer();
  server.enqueue(
    jsonError(400, { error: { message: "bad request", type: "invalid_request_error" } }),
  );
  server.enqueue({ sse: successfulChat() });
  const directory = tempDir();
  try {
    const error = await streamFailure({
      directory: directory.path,
      modelId: "grok-4.6",
      baseUrl: server.baseUrl,
      credential: oauthCredential(),
    });
    assert.equal(error.category, "invalid_request");
    assert.equal(error.httpStatus, 400);
    assert.equal(error.attemptCount, 1);
    assert.equal(server.requests.length, 1);
  } finally {
    await server.close();
    directory.close();
  }
});
test("429 with Retry-After retries and then succeeds without sleeping", async () => {
  const server = await startFakeServer();
  server.enqueue({
    status: 429,
    headers: { "retry-after": "2", "x-request-id": "req_wait" },
    body: JSON.stringify({ error: { message: "rate limited" } }),
  });
  server.enqueue({ sse: successfulChat("after wait") });
  const directory = tempDir();
  const clock = new ManualClock();
  try {
    const pending = runStream({
      directory: directory.path,
      modelId: "grok-4.6",
      baseUrl: server.baseUrl,
      credential: oauthCredential(),
      clock,
    });
    await waitFor(() => clock.pending !== undefined);
    clock.release();
    const records = await pending;
    assert.equal(deltas(records, "text").join(""), "after wait");
    assert.deepEqual(clock.delays, [2_000]);
    assert.equal(server.requests.length, 2);
  } finally {
    await server.close();
    directory.close();
  }
});

test("exhausted 429 reports attempts, status, and request id", async () => {
  const server = await startFakeServer();
  for (let index = 0; index < 3; index += 1) {
    server.enqueue({
      status: 429,
      headers: { "retry-after": "1", "x-request-id": "req_exhausted" },
      body: JSON.stringify({ error: { message: "rate limited" } }),
    });
  }
  const directory = tempDir();
  const clock = new ManualClock();
  try {
    const error = await streamFailure({
      directory: directory.path,
      modelId: "grok-4.6",
      baseUrl: server.baseUrl,
      credential: oauthCredential(),
      clock,
      releases: 2,
    });
    assert.equal(error.category, "rate_limited");
    assert.equal(error.attemptCount, 3);
    assert.equal(error.httpStatus, 429);
    assert.equal(error.requestId, "req_exhausted");
    assert.match(error.message, /after 3 attempts/);
    assert.equal(server.requests.length, 3);
  } finally {
    await server.close();
    directory.close();
  }
});

test("500 and 503 responses are retried", async () => {
  const server = await startFakeServer();
  server.enqueue(jsonError(500, { error: { message: "unavailable" } }));
  server.enqueue({
    status: 503,
    headers: { "retry-after": "1" },
    body: JSON.stringify({ error: { message: "unavailable" } }),
  });
  server.enqueue({ sse: successfulChat("recovered") });
  const directory = tempDir();
  const clock = new ManualClock();
  try {
    const records = await runStream({
      directory: directory.path,
      modelId: "grok-4.6",
      baseUrl: server.baseUrl,
      credential: oauthCredential(),
      clock,
      releases: 2,
    });
    assert.equal(deltas(records, "text").join(""), "recovered");
    assert.equal(server.requests.length, 3);
  } finally {
    await server.close();
    directory.close();
  }
});

test("connection reset after the complete request is read stays unknown across retries", async () => {
  const server = await startFakeServer();
  server.enqueue({ reset: true });
  server.enqueue({ reset: true });
  server.enqueue({ reset: true });
  const directory = tempDir();
  const clock = new ManualClock();
  try {
    const records: WireStreamRecord[] = [];
    const error = await streamFailure({
      directory: directory.path,
      modelId: "grok-4.6",
      baseUrl: server.baseUrl,
      credential: oauthCredential(),
      clock,
      releases: 2,
      emit: (record) => {
        records.push(record);
      },
    });
    assert.equal(error.category, "network");
    assert.equal(error.attemptCount, 3);
    assert.equal(error.httpStatus, undefined);
    assert.equal(error.causeCode, "UND_ERR_SOCKET");
    assert.equal(error.inferenceOutcome, "unknown");
    assert.equal(error.retryable, true);
    assert.match(error.message, /connection reset after the request may have been transmitted \(UND_ERR_SOCKET\)/);
    assert.equal(server.requests.length, 3);
    for (const request of server.requests) {
      assert.equal(request.method, "POST");
      assert.ok(request.body.length > 0);
      const body = JSON.parse(request.body) as { model?: string };
      assert.equal(body.model, "grok-4.6");
    }
    const retries = records.filter((record) => record.event === "retry_attempt");
    assert.equal(retries.length, 2);
    assert.equal(retries[0]?.event, "retry_attempt");
    if (retries[0]?.event === "retry_attempt") {
      assert.equal(retries[0].attempt, 1);
      assert.equal(retries[0].next_attempt, 2);
      assert.equal(retries[0].category, "network");
    }
  } finally {
    await server.close();
    directory.close();
  }
});

test("malformed SSE is a protocol failure", async () => {
  const server = await startFakeServer();
  server.enqueue({ sse: ["data: {not-json"] });
  const directory = tempDir();
  try {
    const error = await streamFailure({
      directory: directory.path,
      modelId: "grok-4.6",
      baseUrl: server.baseUrl,
      credential: oauthCredential(),
    });
    assert.equal(error.category, "protocol", error.message);
    assert.equal(error.attemptCount, 1);
  } finally {
    await server.close();
    directory.close();
  }
});

test("stream interruption after output is not retried and outcome is unknown", async () => {
  const server = await startFakeServer();
  server.enqueue({
    partialSse: [
      'data: {"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":"partial"},"finish_reason":null}]}',
    ],
  });
  const directory = tempDir();
  try {
    const records: WireStreamRecord[] = [];
    const error = await streamFailure({
      directory: directory.path,
      modelId: "grok-4.6",
      baseUrl: server.baseUrl,
      credential: oauthCredential(),
      emit: (record) => {
        records.push(record);
      },
    });
    assert.ok(records.some((record) => record.event === "content_delta"));
    assert.equal(error.inferenceOutcome, "unknown");
    assert.equal(error.retryable, false);
    assert.equal(error.attemptCount, 1);
  } finally {
    await server.close();
    directory.close();
  }
});

test("cancellation interrupts an in-flight request and pending backoff", async () => {
  const hanging = await startFakeServer();
  hanging.enqueue({ hang: true });
  const directory = tempDir();
  try {
    const abort = new AbortController();
    const pending = streamFailure({
      directory: directory.path,
      modelId: "grok-4.6",
      baseUrl: hanging.baseUrl,
      credential: oauthCredential(),
      signal: abort.signal,
    });
    await waitFor(() => hanging.requests.length > 0);
    abort.abort();
    const error = await pending;
    assert.equal(error.category, "cancelled");
    assert.equal(error.inferenceOutcome, "unknown");
  } finally {
    await hanging.close();
    directory.close();
  }

  const server = await startFakeServer();
  server.enqueue({
    status: 429,
    headers: { "retry-after": "30" },
    body: JSON.stringify({ error: { message: "rate limited" } }),
  });
  const backoffDir = tempDir();
  const clock = new ManualClock();
  const abort = new AbortController();
  try {
    const pending = streamFailure({
      directory: backoffDir.path,
      modelId: "grok-4.6",
      baseUrl: server.baseUrl,
      credential: oauthCredential(),
      clock,
      signal: abort.signal,
    });
    await waitFor(() => clock.pending !== undefined);
    abort.abort();
    const error = await pending;
    assert.equal(error.category, "cancelled");
    assert.equal(error.inferenceOutcome, "known_not_started");
    assert.equal(server.requests.length, 1);
  } finally {
    await server.close();
    backoffDir.close();
  }
});

test("status-less errors after fetch dispatch stay unknown", async () => {
  const server = await startFakeServer();
  const directory = tempDir();
  try {
    const error = await streamFailure({
      directory: directory.path,
      modelId: "grok-4.6",
      baseUrl: server.baseUrl,
      credential: oauthCredential(),
      fetch: async () => {
        throw new Error("invalid api key");
      },
    });
    assert.equal(error.httpStatus, undefined);
    assert.equal(error.inferenceOutcome, "unknown");
    assert.notEqual(error.inferenceOutcome, "known_not_started");
  } finally {
    await server.close();
    directory.close();
  }
});

import assert from "node:assert/strict";
import { join } from "node:path";
import { test } from "node:test";

import {
  oauthCredential,
  startFakeServer,
  successfulChat,
  tempDir,
} from "./helpers.js";
import {
  deltas,
  runStream,
  streamFailure,
} from "./stream-support.js";

test("expired OAuth token refreshes before the first request", async () => {
  const server = await startFakeServer();
  server.enqueue({ sse: successfulChat("refreshed") });
  const directory = tempDir();
  const original = globalThis.fetch;
  const storePath = join(directory.path, "credentials.sqlite");
  try {
    globalThis.fetch = async (input, init) => {
      const url = String(input);
      if (url.includes("auth.x.ai")) {
        return new Response(
          JSON.stringify({
            access_token: "access-token-new",
            refresh_token: "refresh-token-new",
            expires_in: 3600,
          }),
          { status: 200, headers: { "content-type": "application/json" } },
        );
      }
      return original(input, init);
    };
    const records = await runStream({
      directory: directory.path,
      modelId: "grok-4.6",
      baseUrl: server.baseUrl,
      credential: oauthCredential(Date.now() - 1_000),
    });
    assert.equal(deltas(records, "text").join(""), "refreshed");
    assert.equal(server.requests.length, 1);
    const { SqliteCredentialStore } = await import("../src/credentials.js");
    const store = new SqliteCredentialStore(storePath);
    const stored = store.read("xai");
    store.close();
    assert.equal(stored?.type, "oauth");
    if (stored?.type === "oauth") {
      assert.equal(stored.access, "access-token-new");
    }
  } finally {
    globalThis.fetch = original;
    await server.close();
    directory.close();
  }
});
test("401 invalid_token refreshes once and retries the request", async () => {
  const server = await startFakeServer();
  server.enqueue({
    status: 401,
    headers: { "www-authenticate": 'Bearer error="invalid_token"' },
    body: JSON.stringify({ error: { message: "invalid_token", code: "invalid_token" } }),
  });
  server.enqueue({ sse: successfulChat("after 401") });
  const directory = tempDir();
  const original = globalThis.fetch;
  try {
    globalThis.fetch = async (input, init) => {
      const url = String(input);
      if (url.includes("auth.x.ai")) {
        return new Response(
          JSON.stringify({
            access_token: "access-token-new",
            refresh_token: "refresh-token-new",
            expires_in: 3600,
          }),
          { status: 200, headers: { "content-type": "application/json" } },
        );
      }
      return original(input, init);
    };
    const records = await runStream({
      directory: directory.path,
      modelId: "grok-4.6",
      baseUrl: server.baseUrl,
      credential: oauthCredential(Date.now() + 60 * 60 * 1000),
    });
    assert.equal(deltas(records, "text").join(""), "after 401");
    assert.equal(server.requests.length, 2);
  } finally {
    globalThis.fetch = original;
    await server.close();
    directory.close();
  }
});

test("401 refresh failure is authentication and does not retry further", async () => {
  const server = await startFakeServer();
  server.enqueue({
    status: 401,
    body: JSON.stringify({ error: { message: "invalid_token", code: "invalid_token" } }),
  });
  const directory = tempDir();
  const original = globalThis.fetch;
  try {
    globalThis.fetch = async (input, init) => {
      const url = String(input);
      if (url.includes("auth.x.ai")) {
        return new Response(JSON.stringify({ error: "invalid_grant" }), { status: 400 });
      }
      return original(input, init);
    };
    const error = await streamFailure({
      directory: directory.path,
      modelId: "grok-4.6",
      baseUrl: server.baseUrl,
      credential: oauthCredential(Date.now() + 60 * 60 * 1000),
    });
    assert.equal(error.category, "authentication");
    assert.equal(error.attemptCount, 1);
    assert.equal(server.requests.length, 1);
  } finally {
    globalThis.fetch = original;
    await server.close();
    directory.close();
  }
});

test("provider request diagnostics redact authorization headers", async () => {
  const server = await startFakeServer();
  server.enqueue({
    sse: successfulChat("ok"),
    headers: { authorization: "Bearer leaked-token", "x-request-id": "req_ok" },
  });
  const directory = tempDir();
  try {
    const records = await runStream({
      directory: directory.path,
      modelId: "grok-4.6",
      baseUrl: server.baseUrl,
      credential: oauthCredential(),
    });
    const encoded = JSON.stringify(records);
    assert.equal(encoded.includes("leaked-token"), false);
    assert.equal(encoded.includes("access-token-old"), false);
    const response = records.find((record) => record.event === "provider_response");
    assert.equal(response?.event, "provider_response");
    if (response?.event === "provider_response") {
      assert.equal(response.headers.authorization, "<redacted>");
    }
  } finally {
    await server.close();
    directory.close();
  }
});

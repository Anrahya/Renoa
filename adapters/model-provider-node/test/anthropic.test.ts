import assert from "node:assert/strict";
import { join } from "node:path";
import { test } from "node:test";

import { findCatalogModel, validateModelSpec } from "../src/catalog.js";
import { SqliteCredentialStore } from "../src/credentials.js";
import { loadCatalog } from "../src/runtime.js";
import { anthropicSse, oauthCredential, startFakeServer, tempDir } from "./helpers.js";
import { runStream } from "./stream-support.js";

const ANTHROPIC_KEY = { type: "api_key", key: "anthropic-test-key" } as const;

test("Anthropic advertises Claude Haiku 5.5 on its own API", () => {
  const haiku = findCatalogModel("anthropic", "claude-haiku-5-5");
  assert.ok(haiku);
  assert.equal(haiku.model.api, "anthropic-messages");
  assert.equal(haiku.model.baseUrl, "https://api.anthropic.com");
  assert.equal(haiku.model.contextWindow, 1_000_000);
  assert.equal((haiku.model as { compat?: { forceAdaptiveThinking?: boolean } }).compat?.forceAdaptiveThinking, true);
  assert.ok(findCatalogModel("anthropic", "claude-sonnet-5-5"));
  assert.throws(
    () =>
      validateModelSpec(
        { ...haiku.model, baseUrl: "https://api.example.com" },
        "anthropic",
        "claude-haiku-5-5",
      ),
    /invalid/u,
  );
});

test("Anthropic requires an API key credential", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  try {
    const store = new SqliteCredentialStore(path);
    store.write("anthropic", oauthCredential());
    store.close();
    await assert.rejects(loadCatalog("anthropic", path), /Anthropic requires an API key/u);
  } finally {
    directory.close();
  }
});

test("Haiku 5.5 sends the key, Renoa's identity, adaptive thinking, and drops stale reasoning", async () => {
  const server = await startFakeServer();
  server.enqueue({ sse: anthropicSse("hi") });
  const directory = tempDir();
  try {
    const records = await runStream({
      directory: directory.path,
      provider: "anthropic",
      modelId: "claude-haiku-5-5",
      baseUrl: server.origin,
      credential: ANTHROPIC_KEY,
      reasoningLevel: "medium",
    });
    assert.ok(records.some((record) => record.event === "completed"));
    const request = server.requests[0];
    assert.ok(request);
    assert.equal(request.url, "/v1/messages?beta=true");
    assert.equal(request.headers["x-api-key"], "anthropic-test-key");
    assert.equal(request.headers["user-agent"], "renoa/0.1.0");
    const body = JSON.parse(request.body) as {
      model?: string;
      thinking?: { type?: string; block_binding?: { prefix_mismatch_behavior?: string } };
      messages?: { role?: string; output_config?: { effort?: string } }[];
    };
    assert.equal(body.model, "claude-haiku-5-5");
    assert.equal(body.thinking?.type, "adaptive");
    assert.equal(body.thinking?.block_binding?.prefix_mismatch_behavior, "drop_block");
    // Pi sets the turn's effort with a trailing system message, so the cached
    // prefix stays the same when the level changes.
    const marker = body.messages?.findLast((message) => message.role === "system");
    assert.equal(marker?.output_config?.effort, "medium");
  } finally {
    await server.close();
    directory.close();
  }
});

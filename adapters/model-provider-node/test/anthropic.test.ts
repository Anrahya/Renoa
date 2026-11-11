import assert from "node:assert/strict";
import { join } from "node:path";
import { test } from "node:test";

import { findCatalogModel, loadPinnedCatalog, validateModelSpec } from "../src/catalog.js";
import { SqliteCredentialStore } from "../src/credentials.js";
import { loadCatalog } from "../src/runtime.js";
import { anthropicSse, oauthCredential, startFakeServer, tempDir } from "./helpers.js";
import { runStream, streamFailure } from "./stream-support.js";

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
      output_config?: { effort?: string };
      messages?: { role?: string }[];
      tools?: { name?: string }[];
    };
    assert.equal(body.model, "claude-haiku-5-5");
    assert.equal(body.thinking?.type, "adaptive");
    assert.equal(body.thinking?.block_binding?.prefix_mismatch_behavior, "drop_block");
    // One effort for the request and the one tool list Renoa sent: no
    // mid-conversation effort markers, deferred-tool placeholder, or the betas
    // that carry them.
    assert.equal(body.output_config?.effort, "medium");
    assert.deepEqual(body.messages?.map((message) => message.role), ["user"]);
    assert.deepEqual(body.tools?.map((tool) => tool.name), ["lookup"]);
    const betas = String(request.headers["anthropic-beta"] ?? "");
    assert.doesNotMatch(betas, /inline-tools|mid-conversation|fallback/u);
  } finally {
    await server.close();
    directory.close();
  }
});

test("Anthropic catalog entries carry no compat switch Renoa's contract cannot honour", () => {
  for (const entry of loadPinnedCatalog("anthropic")) {
    const compat = (entry.model as { compat?: Record<string, unknown> }).compat ?? {};
    for (const field of [
      "supportsMidConvoEffort",
      "supportsMidConvoSystemMessages",
      "supportsMidConvoToolAdditions",
      "supportsMidConvoToolChanges",
      "supportsAdditionalTools",
      "allowedFallbackModels",
    ]) {
      assert.equal(compat[field], undefined, `${entry.id} ${field}`);
    }
  }
});

test("Anthropic refuses a stored subscription token before any request", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  try {
    const store = new SqliteCredentialStore(path);
    store.write("anthropic", { type: "api_key", key: "sk-ant-oat01-token" });
    store.close();
    await assert.rejects(loadCatalog("anthropic", path), /not a subscription token/u);
  } finally {
    directory.close();
  }
});

test("a tool call whose arguments are not JSON fails the turn instead of running with none", async () => {
  const server = await startFakeServer();
  server.enqueue({ sse: brokenToolCallSse() });
  const directory = tempDir();
  try {
    const failure = await streamFailure({
      directory: directory.path,
      provider: "anthropic",
      modelId: "claude-haiku-5-5",
      baseUrl: server.origin,
      credential: ANTHROPIC_KEY,
    });
    assert.equal(failure.category, "protocol");
    assert.match(failure.providerMessage ?? "", /lookup tool-call arguments are not valid JSON/u);
  } finally {
    await server.close();
    directory.close();
  }
});

function brokenToolCallSse(): string[] {
  const frame = (name: string, data: unknown) => `event: ${name}\ndata: ${JSON.stringify(data)}`;
  return [
    frame("message_start", {
      type: "message_start",
      message: {
        id: "msg_1",
        type: "message",
        role: "assistant",
        content: [],
        model: "claude-haiku-5-5",
        stop_reason: null,
        stop_sequence: null,
        usage: { input_tokens: 9, output_tokens: 0 },
      },
    }),
    frame("content_block_start", {
      type: "content_block_start",
      index: 0,
      content_block: { type: "tool_use", id: "toolu_1", name: "lookup", input: {} },
    }),
    frame("content_block_delta", {
      type: "content_block_delta",
      index: 0,
      delta: { type: "input_json_delta", partial_json: '{"q":"a' },
    }),
    frame("content_block_stop", { type: "content_block_stop", index: 0 }),
    frame("message_delta", {
      type: "message_delta",
      delta: { stop_reason: "tool_use", stop_sequence: null },
      usage: { output_tokens: 3 },
    }),
    frame("message_stop", { type: "message_stop" }),
  ];
}

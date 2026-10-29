import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

import { findCatalogModel, loadPinnedCatalog, validateModelSpec } from "../src/catalog.js";
import { loadBridgeConfig } from "../src/config.js";
import type { WireModelRequest } from "../src/contract.js";
import { SqliteCredentialStore } from "../src/credentials.js";
import { loadCatalog } from "../src/runtime.js";
import { oauthCredential, startFakeServer, successfulChat, tempDir } from "./helpers.js";
import { runStream } from "./stream-support.js";

const DEEPSEEK_KEY = { type: "api_key", key: "deepseek-test-key" } as const;
const bridge = fileURLToPath(new URL("../src/main.js", import.meta.url));

test("DeepSeek advertises V4.1 Flash and V4 Pro on its own API", () => {
  const catalog = loadPinnedCatalog("deepseek");
  assert.deepEqual(
    catalog.map((entry) => entry.id),
    ["deepseek-v4-pro", "deepseek-flash"],
  );
  const flash = findCatalogModel("deepseek", "deepseek-flash");
  assert.ok(flash);
  assert.equal(flash.name, "DeepSeek V4.1 Flash");
  assert.equal(flash.model.api, "openai-completions");
  assert.equal(flash.model.baseUrl, "https://api.deepseek.com");
  assert.deepEqual(flash.model.input, ["text", "image"]);
  assert.equal(flash.model.contextWindow, 1_000_000);
  assert.equal(flash.model.maxTokens, 384_000);
  assert.deepEqual([...flash.reasoning_levels], ["off", "low", "high", "max"]);
  const pro = findCatalogModel("deepseek", "deepseek-v4-pro");
  assert.deepEqual([...(pro?.reasoning_levels ?? [])], ["off", "high", "max"]);
  assert.throws(
    () => validateModelSpec({ ...flash.model, baseUrl: "https://api.example.com" }, "deepseek", "deepseek-flash"),
    /invalid/u,
  );
  assert.throws(
    () => validateModelSpec({ ...flash.model, api: "openai-responses" }, "deepseek", "deepseek-flash"),
    /invalid/u,
  );
});

test("DeepSeek requires an API key credential", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  try {
    const store = new SqliteCredentialStore(path);
    store.write("deepseek", oauthCredential());
    store.close();
    await assert.rejects(loadCatalog("deepseek", path), /DeepSeek requires an API key/u);
  } finally {
    directory.close();
  }
});

test("DeepSeek sends max_tokens, the deepseek thinking parameter, and replays reasoning_content", async () => {
  const request: WireModelRequest = {
    system_prompt: "You are a test model.",
    messages: [
      { role: "user", content: [{ type: "text", text: "Look it up." }] },
      {
        role: "assistant",
        content: [
          { type: "reasoning", text: "check the city", signature: "reasoning_content", redacted: false },
          { type: "tool_call", id: "call_1", name: "lookup", arguments: { q: "city" } },
        ],
        stop_reason: "tool_use",
        usage: null,
        metadata: { api: "openai-completions", provider: "deepseek", model: "deepseek-flash" },
      },
      {
        role: "tool",
        result: { call_id: "call_1", name: "lookup", content: [{ type: "text", text: "Paris" }], details: null, is_error: false },
      },
    ],
    tools: [
      {
        name: "lookup",
        description: "Look up a value.",
        input_schema: { type: "object", properties: { q: { type: "string" } }, required: ["q"] },
      },
    ],
  };
  const server = await startFakeServer();
  server.enqueue({ sse: successfulChat("high") });
  server.enqueue({ sse: successfulChat("off") });
  const directory = tempDir();
  try {
    for (const reasoningLevel of ["high", "off"] as const) {
      await runStream({
        directory: directory.path,
        provider: "deepseek",
        modelId: "deepseek-flash",
        baseUrl: server.origin,
        credential: DEEPSEEK_KEY,
        reasoningLevel,
        request,
      });
    }
    const [high, off] = server.requests;
    assert.ok(high && off);
    assert.equal(high.method, "POST");
    assert.equal(high.url, "/chat/completions");
    assert.equal(high.headers.authorization, "Bearer deepseek-test-key");
    const body = JSON.parse(high.body) as Record<string, unknown>;
    assert.equal(body.model, "deepseek-flash");
    assert.equal(typeof body.max_tokens, "number");
    assert.equal(body.max_completion_tokens, undefined);
    assert.equal(body.store, undefined);
    assert.deepEqual(body.thinking, { type: "enabled" });
    assert.equal(body.reasoning_effort, "high");
    const messages = body.messages as { role: string; reasoning_content?: string }[];
    assert.equal(messages[0]?.role, "system");
    const assistant = messages.find((message) => message.role === "assistant");
    assert.equal(assistant?.reasoning_content, "check the city");

    const offBody = JSON.parse(off.body) as Record<string, unknown>;
    assert.deepEqual(offBody.thinking, { type: "disabled" });
    assert.equal(offBody.reasoning_effort, undefined);
  } finally {
    await server.close();
    directory.close();
  }
});

test("the bridge accepts deepseek and refuses an unknown provider without relabelling it", async () => {
  assert.equal(
    loadBridgeConfig({
      RENOA_MODEL_ACTION: "catalog",
      RENOA_MODEL_PROVIDER: "deepseek",
      RENOA_MODEL_AUTH_STORE: "/tmp/credentials.sqlite",
    }).provider,
    "deepseek",
  );
  assert.throws(
    () =>
      loadBridgeConfig({
        RENOA_MODEL_ACTION: "catalog",
        RENOA_MODEL_PROVIDER: "anthropic",
        RENOA_MODEL_AUTH_STORE: "/tmp/credentials.sqlite",
      }),
    /RENOA_MODEL_PROVIDER must be one of xai, opencode-go, deepseek/u,
  );

  const directory = tempDir();
  try {
    const authStore = join(directory.path, "credentials.sqlite");
    const store = new SqliteCredentialStore(authStore);
    store.write("deepseek", DEEPSEEK_KEY);
    store.close();
    const catalog = await runBridge(
      { RENOA_MODEL_ACTION: "catalog", RENOA_MODEL_PROVIDER: "deepseek", RENOA_MODEL_AUTH_STORE: authStore },
      "",
    );
    assert.equal(catalog.status, 0, catalog.stderr);
    const models = (JSON.parse(catalog.stdout) as { response: { models: { id: string }[] } }).response.models;
    assert.deepEqual(models.map((model) => model.id), ["deepseek-v4-pro", "deepseek-flash"]);

    const unknown = await runBridge(
      {
        RENOA_MODEL_ACTION: "stream",
        RENOA_MODEL_PROVIDER: "anthropic",
        RENOA_MODEL: "claude",
        RENOA_MODEL_AUTH_STORE: authStore,
        RENOA_MODEL_MAX_OUTPUT_TOKENS: "128",
      },
      "{}",
    );
    assert.equal(unknown.status, 1);
    const record = JSON.parse(unknown.stdout.trim()) as {
      event?: string;
      error?: string;
      diagnostic?: { provider?: string };
    };
    assert.equal(record.event, "error");
    assert.match(record.error ?? "", /RENOA_MODEL_PROVIDER must be one of/u);
    assert.equal(record.diagnostic?.provider, "unknown");
  } finally {
    directory.close();
  }
});

function runBridge(
  env: NodeJS.ProcessEnv,
  input: string,
): Promise<{ status: number | null; stdout: string; stderr: string }> {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, ["--disable-warning=ExperimentalWarning", bridge], {
      env: { ...process.env, ...env },
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
    });
    child.on("close", (status) => resolve({ status, stdout, stderr }));
    child.stdin.end(input);
  });
}

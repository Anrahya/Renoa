import assert from "node:assert/strict";
import { join } from "node:path";
import { test } from "node:test";

import { findCatalogModel, loadPinnedCatalog, validateModelSpec } from "../src/catalog.js";
import type { WireModelRequest } from "../src/contract.js";
import { SqliteCredentialStore } from "../src/credentials.js";
import { loadCatalog } from "../src/runtime.js";
import { oauthCredential, startFakeServer, successfulChat, tempDir } from "./helpers.js";
import { runStream } from "./stream-support.js";

const QWEN_KEY = { type: "api_key", key: "qwen-token-plan-test-key" } as const;
const BASE_URL = "https://token-plan.maas.qwencloudapi.com/compatible-mode/v1";

test("Qwen Token Plan Individual advertises Qwen4 Max beside Pi's catalog on its official gateway", () => {
  const catalog = loadPinnedCatalog("qwen-token-plan-individual");
  assert.ok(catalog.some((entry) => entry.id === "qwen3.8-max"));
  assert.ok(catalog.some((entry) => entry.id === "qwen4-max-preview"));

  const qwen4 = findCatalogModel("qwen-token-plan-individual", "qwen4-max");
  assert.ok(qwen4);
  assert.equal(qwen4.name, "Qwen4 Max");
  assert.equal(qwen4.model.api, "openai-completions");
  assert.equal(qwen4.model.provider, "qwen-token-plan-individual");
  assert.equal(qwen4.model.baseUrl, BASE_URL);
  assert.deepEqual(qwen4.model.input, ["text", "image"]);
  assert.equal(qwen4.model.contextWindow, 1_000_000);
  assert.equal(qwen4.model.maxTokens, 262_144);
  assert.deepEqual([...qwen4.reasoning_levels], ["off", "low", "high", "xhigh"]);

  assert.throws(
    () =>
      validateModelSpec(
        { ...qwen4.model, baseUrl: "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1" },
        "qwen-token-plan-individual",
        "qwen4-max",
      ),
    /invalid/u,
  );
  assert.throws(
    () =>
      validateModelSpec(
        { ...qwen4.model, api: "anthropic-messages" },
        "qwen-token-plan-individual",
        "qwen4-max",
      ),
    /invalid/u,
  );
});

test("Qwen Token Plan Individual requires an API key credential", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  try {
    const store = new SqliteCredentialStore(path);
    store.write("qwen-token-plan-individual", oauthCredential());
    store.close();
    await assert.rejects(
      loadCatalog("qwen-token-plan-individual", path),
      /Qwen Token Plan Individual requires an API key/u,
    );
  } finally {
    directory.close();
  }
});

test("Qwen4 Max sends Qwen thinking controls, Renoa's identity, system role, and replays reasoning_content", async () => {
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
        metadata: {
          api: "openai-completions",
          provider: "qwen-token-plan-individual",
          model: "qwen4-max",
        },
      },
      {
        role: "tool",
        result: {
          call_id: "call_1",
          name: "lookup",
          content: [{ type: "text", text: "Paris" }],
          details: null,
          is_error: false,
        },
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
  server.enqueue({ sse: successfulChat("xhigh") });
  server.enqueue({ sse: successfulChat("off") });
  const directory = tempDir();
  try {
    for (const reasoningLevel of ["high", "xhigh", "off"] as const) {
      await runStream({
        directory: directory.path,
        provider: "qwen-token-plan-individual",
        modelId: "qwen4-max",
        baseUrl: server.origin,
        credential: QWEN_KEY,
        reasoningLevel,
        maxOutputTokens: 65_536,
        request,
      });
    }
    const [high, xhigh, off] = server.requests;
    assert.ok(high && xhigh && off);
    assert.equal(high.method, "POST");
    assert.equal(high.url, "/chat/completions");
    assert.equal(high.headers.authorization, "Bearer qwen-token-plan-test-key");
    assert.equal(high.headers["user-agent"], "renoa/0.1.0");

    const highBody = JSON.parse(high.body) as Record<string, unknown>;
    assert.equal(highBody.model, "qwen4-max");
    assert.equal(highBody.max_completion_tokens, 65_536);
    assert.equal(highBody.store, undefined);
    assert.equal(highBody.enable_thinking, true);
    assert.equal(highBody.reasoning_effort, "high");
    const messages = highBody.messages as { role: string; reasoning_content?: string }[];
    assert.equal(messages[0]?.role, "system");
    assert.equal(messages.some((message) => message.role === "developer"), false);
    const assistant = messages.find((message) => message.role === "assistant");
    assert.equal(assistant?.reasoning_content, "check the city");

    const xhighBody = JSON.parse(xhigh.body) as Record<string, unknown>;
    assert.equal(xhighBody.enable_thinking, true);
    assert.equal(xhighBody.reasoning_effort, "xhigh");

    const offBody = JSON.parse(off.body) as Record<string, unknown>;
    assert.equal(offBody.enable_thinking, false);
    assert.equal(offBody.reasoning_effort, undefined);
  } finally {
    await server.close();
    directory.close();
  }
});

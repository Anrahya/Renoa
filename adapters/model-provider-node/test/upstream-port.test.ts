import assert from "node:assert/strict";
import { test } from "node:test";

import type { WireMessage, WireModelRequest } from "../src/contract.js";
import { isContextOverflow } from "../src/upstream/overflow.js";
import type { AssistantMessage } from "../src/upstream/types.js";
import {
  anthropicSse,
  chatChunk,
  oauthCredential,
  responsesSse,
  sseChat,
  startFakeServer,
  successfulChat,
  tempDir,
} from "./helpers.js";
import { completedRecord, runStream, streamFailure, type StreamOptions } from "./stream-support.js";

// Behaviour ported from Pi v1.4.0 that Renoa's retained transports now exhibit.

const OPENCODE_KEY = { type: "api_key", key: "opencode-test-key" } as const;
const PIXEL = "iVBORw0KGgo=";

test("whitespace-only tool output is sent as the empty-output placeholder", async () => {
  const request = toolTurn([{ type: "text", text: "\r\n" }], false);

  const chat = await captureRequest(
    { provider: "opencode-go", modelId: "glm-5.3", credential: OPENCODE_KEY, request },
    successfulChat("ok"),
  );
  const toolMessage = (chat.messages as { role: string; content: unknown }[]).find(
    (message) => message.role === "tool",
  );
  assert.equal(toolMessage?.content, "(no tool output)");

  const responses = await captureRequest(
    { modelId: "grok-4.6", credential: oauthCredential(), request },
    responsesSse("ok"),
  );
  const output = (responses.input as { type?: string; output?: unknown }[]).find(
    (item) => item.type === "function_call_output",
  );
  assert.equal(output?.output, "(no tool output)");
});

test("tool-call arguments keep every field when a string has an invalid \\u escape", async () => {
  const server = await startFakeServer();
  server.enqueue({
    sse: sseChat([
      chatChunk({
        tool_calls: [{ index: 0, id: "call_tex", type: "function", function: { name: "lookup", arguments: "" } }],
      }),
      chatChunk({
        tool_calls: [{ index: 0, function: { arguments: '{"q":"\\usepackage{amsmath}","n":2}' } }],
      }),
      chatChunk({}, { finish_reason: "tool_calls", usage: { prompt_tokens: 5, completion_tokens: 8 } }),
    ]),
  });
  const directory = tempDir();
  try {
    const records = await runStream({
      directory: directory.path,
      provider: "opencode-go",
      modelId: "glm-5.3",
      baseUrl: server.baseUrl,
      credential: OPENCODE_KEY,
    });
    const tool = completedRecord(records).response.content.find((content) => content.type === "tool_call");
    assert.equal(tool?.type, "tool_call");
    if (tool?.type === "tool_call") {
      assert.deepEqual(tool.arguments, { q: "\\usepackage{amsmath}", n: 2 });
    }
  } finally {
    await server.close();
    directory.close();
  }
});

test("reasoning and answer in one Chat Completions chunk keep reasoning first", async () => {
  const server = await startFakeServer();
  server.enqueue({
    sse: sseChat([
      chatChunk({ reasoning_content: "plan", content: "answer" }),
      chatChunk({}, { finish_reason: "stop", usage: { prompt_tokens: 5, completion_tokens: 3 } }),
    ]),
  });
  const directory = tempDir();
  try {
    const records = await runStream({
      directory: directory.path,
      provider: "opencode-go",
      modelId: "glm-5.3",
      baseUrl: server.baseUrl,
      credential: OPENCODE_KEY,
    });
    assert.deepEqual(
      completedRecord(records).response.content.map((content) => content.type),
      ["reasoning", "text"],
    );
  } finally {
    await server.close();
    directory.close();
  }
});

test("thinking from a different model is dropped instead of replayed as text", async () => {
  const request: WireModelRequest = {
    system_prompt: "You are a test model.",
    messages: [
      { role: "user", content: [{ type: "text", text: "First" }] },
      {
        role: "assistant",
        content: [
          { type: "reasoning", text: "earlier private plan", redacted: false },
          { type: "text", text: "earlier answer" },
        ],
        stop_reason: "stop",
        usage: null,
        metadata: { api: "openai-completions", provider: "opencode-go", model: "glm-5.2" },
      },
      { role: "user", content: [{ type: "text", text: "Second" }] },
    ],
    tools: [],
  };
  const body = await captureRequest(
    { provider: "opencode-go", modelId: "glm-5.3", credential: OPENCODE_KEY, request },
    successfulChat("ok"),
  );
  const serialized = JSON.stringify(body.messages);
  assert.equal(serialized.includes("earlier answer"), true);
  assert.equal(serialized.includes("earlier private plan"), false);
});

test("image-only user messages send no empty text part on Chat Completions", async () => {
  const request: WireModelRequest = {
    system_prompt: "You are a test model.",
    messages: [
      {
        role: "user",
        content: [
          { type: "text", text: "" },
          { type: "image", data: PIXEL, mime_type: "image/png" },
        ],
      },
    ],
    tools: [],
  };
  const body = await captureRequest(
    { provider: "opencode-go", modelId: "kimi-k3", credential: OPENCODE_KEY, request, reasoningLevel: "max" },
    successfulChat("ok"),
  );
  const user = (body.messages as { role: string; content: { type: string }[] }[]).find(
    (message) => message.role === "user",
  );
  assert.deepEqual(
    user?.content.map((part) => part.type),
    ["image_url"],
  );
});

test("top-level cached_tokens is read as cache-read usage", async () => {
  const server = await startFakeServer();
  server.enqueue({
    sse: sseChat([
      chatChunk({ content: "ok" }),
      chatChunk({}, { finish_reason: "stop", usage: { prompt_tokens: 10, completion_tokens: 2, cached_tokens: 7 } }),
    ]),
  });
  const directory = tempDir();
  try {
    const records = await runStream({
      directory: directory.path,
      provider: "opencode-go",
      modelId: "kimi-k3",
      baseUrl: server.baseUrl,
      credential: OPENCODE_KEY,
      reasoningLevel: "max",
    });
    const usage = completedRecord(records).response.usage;
    assert.equal(usage.cache_read, 7);
    assert.equal(usage.input, 3);
  } finally {
    await server.close();
    directory.close();
  }
});

test("Anthropic error tool results carry text only and their images follow the results", async () => {
  const request = toolTurn(
    [
      { type: "text", text: "screenshot failed" },
      { type: "image", data: PIXEL, mime_type: "image/png" },
    ],
    true,
  );
  const body = await captureRequest(
    { provider: "opencode-go", modelId: "minimax-m3", credential: OPENCODE_KEY, request },
    anthropicSse("ok"),
    "origin",
  );
  const messages = body.messages as { role: string; content: { type: string; content?: unknown; text?: string }[] }[];
  const results = messages.at(-1);
  assert.equal(results?.role, "user");
  assert.deepEqual(
    results?.content.map((block) => block.type),
    ["tool_result", "text", "image"],
  );
  const toolResult = results?.content[0];
  assert.equal(JSON.stringify(toolResult?.content).includes("image/png"), false);
  assert.match(results?.content[1]?.text ?? "", /error result of tool call call_1 \(lookup\)/u);
});

test("Claude Haiku 5.5 uses adaptive thinking with an effort and disables thinking at off", async () => {
  const server = await startFakeServer();
  server.enqueue({ sse: anthropicSse("xhigh") });
  server.enqueue({ sse: anthropicSse("off") });
  const directory = tempDir();
  try {
    for (const reasoningLevel of ["xhigh", "off"] as const) {
      await runStream({
        directory: directory.path,
        provider: "opencode-go",
        modelId: "claude-haiku-5-5",
        baseUrl: server.origin,
        credential: OPENCODE_KEY,
        reasoningLevel,
      });
    }
    const [effortRequest, offRequest] = server.requests;
    assert.ok(effortRequest && offRequest);
    const effort = JSON.parse(effortRequest.body) as Record<string, unknown>;
    assert.deepEqual(effort.thinking, { type: "adaptive", display: "summarized" });
    assert.deepEqual(effort.output_config, { effort: "xhigh" });
    assert.equal((effortRequest.headers["anthropic-beta"] ?? "").includes("interleaved-thinking"), false);
    const off = JSON.parse(offRequest.body) as Record<string, unknown>;
    assert.deepEqual(off.thinking, { type: "disabled" });
    assert.equal(off.output_config, undefined);
  } finally {
    await server.close();
    directory.close();
  }
});

test("a Responses stream that completes with an unfinished tool call fails", async () => {
  const server = await startFakeServer();
  server.enqueue({
    sse: [
      sse("response.created", {
        type: "response.created",
        response: { id: "resp_1", model: "grok-4.6", status: "in_progress", output: [] },
      }),
      sse("response.output_item.added", {
        type: "response.output_item.added",
        output_index: 0,
        item: { type: "function_call", id: "fc_1", call_id: "call_1", name: "lookup", arguments: "" },
      }),
      sse("response.function_call_arguments.delta", {
        type: "response.function_call_arguments.delta",
        output_index: 0,
        item_id: "fc_1",
        delta: '{"q":"ci',
      }),
      sse("response.completed", {
        type: "response.completed",
        response: {
          id: "resp_1",
          model: "grok-4.6",
          status: "completed",
          output: [],
          usage: { input_tokens: 4, output_tokens: 2, total_tokens: 6 },
        },
      }),
    ],
  });
  const directory = tempDir();
  try {
    const failure = await streamFailure({
      directory: directory.path,
      modelId: "grok-4.6",
      baseUrl: server.baseUrl,
      credential: oauthCredential(),
    });
    assert.match(failure.message, /unfinished tool call: lookup/u);
  } finally {
    await server.close();
    directory.close();
  }
});

test("HTTP 413 and 'Prompt too long' errors count as context overflow", () => {
  for (const errorMessage of [
    'opencode-go API error (413): {"message":"failed to parse request"}',
    "413 Request Entity Too Large",
    '{"code":"1261","message":"Prompt too long"}',
  ]) {
    assert.equal(isContextOverflow(errorMessageFixture(errorMessage)), true, errorMessage);
  }
  assert.equal(
    isContextOverflow(errorMessageFixture('xai API error (400): {"detail":"limit 413 reached"}')),
    false,
  );
});

function errorMessageFixture(errorMessage: string): AssistantMessage {
  return {
    role: "assistant",
    content: [],
    api: "openai-completions",
    provider: "opencode-go",
    model: "glm-5.3",
    usage: {
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
      totalTokens: 0,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
    },
    stopReason: "error",
    errorMessage,
    timestamp: 0,
  };
}

function toolTurn(
  content: Extract<WireMessage, { role: "tool" }>["result"]["content"],
  isError: boolean,
): WireModelRequest {
  return {
    system_prompt: "You are a test model.",
    messages: [
      { role: "user", content: [{ type: "text", text: "Look it up." }] },
      {
        role: "assistant",
        content: [{ type: "tool_call", id: "call_1", name: "lookup", arguments: { q: "city" } }],
        stop_reason: "tool_use",
        usage: null,
        metadata: { api: "openai-completions", provider: "opencode-go", model: "glm-5.2" },
      },
      {
        role: "tool",
        result: { call_id: "call_1", name: "lookup", content, details: null, is_error: isError },
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
}

async function captureRequest(
  options: Omit<StreamOptions, "directory" | "baseUrl">,
  sse: readonly string[],
  base: "baseUrl" | "origin" = "baseUrl",
): Promise<Record<string, unknown>> {
  const server = await startFakeServer();
  server.enqueue({ sse });
  const directory = tempDir();
  try {
    await runStream({ ...options, directory: directory.path, baseUrl: server[base] });
    const request = server.requests[0];
    assert.ok(request);
    return JSON.parse(request.body) as Record<string, unknown>;
  } finally {
    await server.close();
    directory.close();
  }
}

function sse(name: string, data: unknown): string {
  return `event: ${name}\ndata: ${JSON.stringify(data)}`;
}

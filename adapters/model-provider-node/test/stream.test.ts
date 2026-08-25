import assert from "node:assert/strict";
import { test } from "node:test";

import {
  anthropicSse,
  oauthCredential,
  reasoningChat,
  responsesSse,
  startFakeServer,
  successfulChat,
  tempDir,
  toolChat,
} from "./helpers.js";
import {
  completedRecord,
  deltas,
  runStream,
  withOpenCode,
} from "./stream-support.js";

test("xAI streams text, reasoning, tools, usage, and completion", async () => {
  const server = await startFakeServer();
  server.enqueue({ sse: reasoningChat() });
  const directory = tempDir();
  try {
    const records = await runStream({
      directory: directory.path,
      modelId: "grok-4.6",
      baseUrl: server.baseUrl,
      credential: oauthCredential(),
    });
    const text = deltas(records, "text").join("");
    const reasoning = deltas(records, "reasoning").join("");
    assert.equal(text, "visible");
    assert.equal(reasoning, "plan");
    const completed = completedRecord(records);
    assert.equal(completed.response.stop_reason, "stop");
    assert.equal(completed.response.usage.cache_read, 0);
    assert.equal(completed.response.metadata.provider, "xai");
  } finally {
    await server.close();
    directory.close();
  }

  const toolsServer = await startFakeServer();
  toolsServer.enqueue({ sse: toolChat() });
  const toolsDir = tempDir();
  try {
    const records = await runStream({
      directory: toolsDir.path,
      modelId: "grok-4.6",
      baseUrl: toolsServer.baseUrl,
      credential: oauthCredential(),
    });
    const start = records.find(
      (record) => record.event === "content_delta" && record.delta.type === "tool_call_start",
    );
    assert.equal(start?.event, "content_delta");
    if (start?.event === "content_delta" && start.delta.type === "tool_call_start") {
      assert.equal(start.delta.id, "call_stable");
      assert.equal(start.delta.name, "lookup");
    }
    assert.equal(deltas(records, "tool_call_arguments").join(""), '{"q":"city"}');
    const completed = completedRecord(records);
    const tool = completed.response.content.find((content) => content.type === "tool_call");
    assert.equal(tool?.type, "tool_call");
    if (tool?.type === "tool_call") {
      assert.equal(tool.id, "call_stable");
      assert.deepEqual(tool.arguments, { q: "city" });
    }
  } finally {
    await toolsServer.close();
    toolsDir.close();
  }
});

test("OpenCode Go streams one model for each supported wire API", async () => {
  await withOpenCode("glm-5.1", (server) => {
    server.enqueue({ sse: successfulChat("chat") });
    return server.baseUrl;
  }, (records) => {
    assert.equal(deltas(records, "text").join(""), "chat");
    assert.equal(completedRecord(records).response.metadata.api, "openai-completions");
  });

  await withOpenCode("grok-4.5", (server) => {
    server.enqueue({ sse: responsesSse("responses") });
    return server.baseUrl;
  }, (records) => {
    assert.equal(deltas(records, "text").join(""), "responses");
    assert.equal(completedRecord(records).response.metadata.api, "openai-responses");
    assert.equal(completedRecord(records).response.metadata.response_id, "resp_1");
    assert.equal(completedRecord(records).response.usage.cache_read, 3);
  });

  await withOpenCode("minimax-m3", (server) => {
    server.enqueue({ sse: anthropicSse("anthropic") });
    return server.origin;
  }, (records) => {
    assert.equal(deltas(records, "text").join(""), "anthropic");
    assert.equal(completedRecord(records).response.metadata.api, "anthropic-messages");
  });
});

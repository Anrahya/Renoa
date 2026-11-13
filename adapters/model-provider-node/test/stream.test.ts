import assert from "node:assert/strict";
import { test } from "node:test";

import {
  anthropicSse,
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

test("Chat Completions streams text, reasoning, tools, usage, and completion", async () => {
  const server = await startFakeServer();
  server.enqueue({ sse: reasoningChat() });
  const directory = tempDir();
  try {
    const records = await runStream({
      directory: directory.path,
      provider: "opencode-go",
      modelId: "glm-5.3",
      baseUrl: server.baseUrl,
      credential: { type: "api_key", key: "opencode-test-key" },
    });
    const text = deltas(records, "text").join("");
    const reasoning = deltas(records, "reasoning").join("");
    assert.equal(text, "visible");
    assert.equal(reasoning, "plan");
    const completed = completedRecord(records);
    assert.equal(completed.response.stop_reason, "stop");
    assert.equal(completed.response.usage.cache_read, 0);
    assert.equal(completed.response.metadata.provider, "opencode-go");
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
      provider: "opencode-go",
      modelId: "glm-5.3",
      baseUrl: toolsServer.baseUrl,
      credential: { type: "api_key", key: "opencode-test-key" },
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
  await withOpenCode("glm-5.3", (server) => {
    server.enqueue({ sse: successfulChat("chat") });
    return server.baseUrl;
  }, (records) => {
    assert.equal(deltas(records, "text").join(""), "chat");
    assert.equal(completedRecord(records).response.metadata.api, "openai-completions");
  });

  await withOpenCode("grok-4.7", (server) => {
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

test("only summarized-thinking providers and summary events tag reasoning deltas as summarized", async () => {
  const sseEvent = (type: string, data: Record<string, unknown>) =>
    `event: ${type}\ndata: ${JSON.stringify(data)}`;
  const reasoningFlags = (records: Awaited<ReturnType<typeof runStream>>) =>
    records.flatMap((record) =>
      record.event === "content_delta" && record.delta.type === "reasoning"
        ? [{ text: record.delta.text, summarized: record.delta.summarized }]
        : [],
    );

  await withOpenCode("glm-5.3", (server) => {
    server.enqueue({ sse: reasoningChat("visible", "raw chain") });
    return server.baseUrl;
  }, (records) => {
    assert.deepEqual(reasoningFlags(records), [{ text: "raw chain", summarized: undefined }]);
  });

  await withOpenCode("grok-4.7", (server) => {
    server.enqueue({
      sse: [
        sseEvent("response.created", {
          type: "response.created",
          response: { id: "resp_1", model: "grok-4.7", status: "in_progress", output: [] },
        }),
        sseEvent("response.output_item.added", {
          type: "response.output_item.added",
          output_index: 0,
          item: { type: "reasoning", id: "rs_1", summary: [] },
        }),
        sseEvent("response.reasoning_summary_text.delta", {
          type: "response.reasoning_summary_text.delta",
          item_id: "rs_1",
          output_index: 0,
          summary_index: 0,
          delta: "Checking files",
        }),
        sseEvent("response.reasoning_text.delta", {
          type: "response.reasoning_text.delta",
          item_id: "rs_1",
          output_index: 0,
          content_index: 0,
          delta: "private chain-of-thought",
        }),
        sseEvent("response.output_item.done", {
          type: "response.output_item.done",
          output_index: 0,
          item: { type: "reasoning", id: "rs_1", summary: [{ type: "summary_text", text: "Checking files" }] },
        }),
        sseEvent("response.completed", {
          type: "response.completed",
          response: {
            id: "resp_1",
            model: "grok-4.7",
            status: "completed",
            output: [{ type: "reasoning", id: "rs_1", summary: [{ type: "summary_text", text: "Checking files" }] }],
            usage: { input_tokens: 5, output_tokens: 2, total_tokens: 7 },
          },
        }),
      ],
    });
    return server.baseUrl;
  }, (records) => {
    assert.deepEqual(reasoningFlags(records), [
      { text: "Checking files", summarized: true },
      { text: "private chain-of-thought", summarized: undefined },
    ]);
  });

  const anthropicThinkingSse = (model: string, thinking: string) => [
    sseEvent("message_start", {
      type: "message_start",
      message: {
        id: "msg_1",
        type: "message",
        role: "assistant",
        content: [],
        model,
        stop_reason: null,
        stop_sequence: null,
        usage: { input_tokens: 9, output_tokens: 0, cache_read_input_tokens: 0, cache_creation_input_tokens: 0 },
      },
    }),
    sseEvent("content_block_start", {
      type: "content_block_start",
      index: 0,
      content_block: { type: "thinking", thinking: "" },
    }),
    sseEvent("content_block_delta", {
      type: "content_block_delta",
      index: 0,
      delta: { type: "thinking_delta", thinking },
    }),
    sseEvent("content_block_stop", { type: "content_block_stop", index: 0 }),
    sseEvent("message_delta", {
      type: "message_delta",
      delta: { stop_reason: "end_turn", stop_sequence: null },
      usage: { output_tokens: 3 },
    }),
    sseEvent("message_stop", { type: "message_stop" }),
  ];

  // Non-Claude models routed via anthropic-messages stream raw thinking, not summaries.
  await withOpenCode("minimax-m3", (server) => {
    server.enqueue({ sse: anthropicThinkingSse("minimax-m3", "Raw minimax scratchpad") });
    return server.origin;
  }, (records) => {
    assert.deepEqual(reasoningFlags(records), [
      { text: "Raw minimax scratchpad", summarized: undefined },
    ]);
  });

  const anthropicServer = await startFakeServer();
  anthropicServer.enqueue({ sse: anthropicThinkingSse("claude-haiku-5-5", "Inspecting tests") });
  const anthropicDir = tempDir();
  try {
    const records = await runStream({
      directory: anthropicDir.path,
      provider: "anthropic",
      modelId: "claude-haiku-5-5",
      baseUrl: anthropicServer.origin,
      credential: { type: "api_key", key: "anthropic-test-key" },
      reasoningLevel: "medium",
    });
    assert.deepEqual(reasoningFlags(records), [
      { text: "Inspecting tests", summarized: true },
    ]);
  } finally {
    await anthropicServer.close();
    anthropicDir.close();
  }
});

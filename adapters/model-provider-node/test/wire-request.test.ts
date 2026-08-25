import assert from "node:assert/strict";
import { test } from "node:test";

import { parseWireModelRequest } from "../src/wire-request.js";

test("inbound WireModelRequest rejects missing fields instead of inventing them", () => {
  assert.throws(() => parseWireModelRequest(null), /JSON object/);
  assert.throws(() => parseWireModelRequest({ messages: [], tools: [] }), /system_prompt/);
  assert.throws(
    () => parseWireModelRequest({ system_prompt: "", tools: [] }),
    /messages/,
  );
  assert.throws(
    () => parseWireModelRequest({ system_prompt: "", messages: [{ role: "system" }], tools: [] }),
    /messages\[0\] is malformed/,
  );
  const request = parseWireModelRequest({
    system_prompt: "Be precise.",
    messages: [{ role: "user", content: [{ type: "text", text: "Hello" }] }],
    tools: [{ name: "read_file", description: "Read a file", input_schema: {} }],
  });
  assert.equal(request.system_prompt, "Be precise.");
  assert.equal(request.messages[0]?.role, "user");
  assert.equal(request.tools[0]?.name, "read_file");
});

test("inbound assistant reasoning defaults an omitted redacted flag to false", () => {
  const request = parseWireModelRequest({
    system_prompt: "Be precise.",
    messages: [
      {
        role: "assistant",
        content: [{ type: "reasoning", text: "Inspect the requested file." }],
        stop_reason: "tool_use",
        usage: { input: 1, output: 1, cache_read: 0, cache_write: 0 },
        metadata: {
          api: "openai-completions",
          provider: "opencode-go",
          model: "ox-alpha-free",
        },
      },
    ],
    tools: [],
  });

  const message = request.messages[0];
  assert.equal(message?.role, "assistant");
  assert.deepEqual(message?.role === "assistant" ? message.content : undefined, [
    { type: "reasoning", text: "Inspect the requested file.", redacted: false },
  ]);
});

test("nested WireModelRequest fields are classified invalid_request instead of throwing TypeError", () => {
  const classified = (value: unknown) => {
    try {
      parseWireModelRequest(value);
      throw new Error("expected invalid request");
    } catch (error) {
      assert.equal(error instanceof TypeError, false, String(error));
      assert.equal((error as { categoryHint?: string }).categoryHint, "invalid_request");
      return error as Error;
    }
  };
  classified({
    system_prompt: "x",
    messages: [{ role: "user", content: "text" }],
    tools: [],
  });
  classified({
    system_prompt: "x",
    messages: [{ role: "user", content: [{ type: "text", text: 1 }] }],
    tools: [],
  });
  classified({
    system_prompt: "x",
    messages: [
      {
        role: "assistant",
        content: [{ type: "tool_call", id: "1", name: "lookup", arguments: ["not-object"] }],
        stop_reason: "tool_use",
        usage: { input: 1, output: 1, cache_read: 0, cache_write: 0 },
        metadata: { api: "openai-completions", provider: "xai", model: "grok-4.6" },
      },
    ],
    tools: [],
  });
  classified({
    system_prompt: "x",
    messages: [
      {
        role: "assistant",
        content: [{ type: "text", text: "hi" }],
        stop_reason: "stop",
        usage: { input: Number.POSITIVE_INFINITY, output: 1, cache_read: 0, cache_write: 0 },
        metadata: { api: "openai-completions", provider: "xai", model: "grok-4.6" },
      },
    ],
    tools: [],
  });
  classified({
    system_prompt: "x",
    messages: [
      {
        role: "tool",
        result: { call_id: "1", name: "lookup", content: [], details: undefined, is_error: "no" },
      },
    ],
    tools: [],
  });
  classified({
    system_prompt: "x",
    messages: [],
    tools: [{ name: "lookup", description: "d", input_schema: ["not-object"] }],
  });
  classified({
    system_prompt: "x",
    messages: [
      {
        role: "assistant",
        content: [{ type: "reasoning", text: "plan", redacted: "no" }],
        stop_reason: "stop",
        usage: { input: 1, output: 1, cache_read: 0, cache_write: 0 },
        metadata: { api: "openai-completions", provider: "xai", model: "grok-4.6" },
      },
    ],
    tools: [],
  });
  classified({
    system_prompt: "x",
    messages: [
      {
        role: "assistant",
        content: [{ type: "text", text: "hi" }],
        stop_reason: "stop",
        usage: { input: 1, output: 1, cache_read: 0, cache_write: 0 },
        metadata: { api: "openai", provider: "xai", model: "grok-4.6" },
      },
    ],
    tools: [],
  });
});

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  fauxAssistantMessage,
  fauxProvider,
  fauxThinking,
  fauxToolCall,
} from "@earendil-works/pi-ai";

import { invokeModel } from "../src/model-bridge.js";

test("the model bridge preserves Renoa input and returns a complete tool response", async () => {
  const faux = fauxProvider();
  faux.setResponses([
    (context) => {
      assert.equal(context.systemPrompt, "Use tools carefully.");
      assert.deepEqual(context.messages, [
        {
          role: "user",
          content: [{ type: "text", text: "Read value.txt." }],
          timestamp: 0,
        },
      ]);
      assert.deepEqual(context.tools, [
        {
          name: "read_file",
          description: "Read one file.",
          parameters: {
            type: "object",
            properties: { path: { type: "string" } },
            required: ["path"],
          },
        },
      ]);
      return fauxAssistantMessage(
        fauxToolCall("read_file", { path: "value.txt" }, { id: "read-1" }),
        { stopReason: "toolUse", responseId: "response-1", timestamp: 10 },
      );
    },
  ]);

  const response = await invokeModel(
    {
      system_prompt: "Use tools carefully.",
      messages: [
        {
          role: "user",
          content: [{ type: "text", text: "Read value.txt." }],
        },
      ],
      tools: [
        {
          name: "read_file",
          description: "Read one file.",
          input_schema: {
            type: "object",
            properties: { path: { type: "string" } },
            required: ["path"],
          },
        },
      ],
    },
    {
      model: faux.getModel(),
      streamFn: faux.provider.streamSimple.bind(faux.provider),
    },
  );

  assert.equal(response.stop_reason, "tool_use");
  assert.deepEqual(response.content, [
    {
      type: "tool_call",
      id: "read-1",
      name: "read_file",
      arguments: { path: "value.txt" },
    },
  ]);
  assert.equal(response.metadata.response_id, "response-1");
  assert.equal(response.metadata.provider, "faux");
  assert.ok(response.usage);
});

test("the model bridge preserves assistant and tool-result continuation context", async () => {
  const faux = fauxProvider();
  faux.setResponses([
    (context) => {
      assert.deepEqual(
        context.messages.map((message) => message.role),
        ["user", "assistant", "toolResult"],
      );
      const assistant = context.messages[1];
      assert.ok(assistant?.role === "assistant");
      assert.deepEqual(assistant.content, [
        { type: "toolCall", id: "read-1", name: "read_file", arguments: { path: "value.txt" } },
      ]);
      const result = context.messages[2];
      assert.ok(result?.role === "toolResult");
      assert.equal(result.toolCallId, "read-1");
      assert.equal(result.isError, false);
      return fauxAssistantMessage([fauxThinking("checked"), { type: "text", text: "Done." }]);
    },
  ]);

  const response = await invokeModel(
    {
      system_prompt: "Continue carefully.",
      messages: [
        { role: "user", content: [{ type: "text", text: "Read it." }] },
        {
          role: "assistant",
          content: [
            {
              type: "tool_call",
              id: "read-1",
              name: "read_file",
              arguments: { path: "value.txt" },
            },
          ],
          stop_reason: "tool_use",
          usage: null,
          metadata: { api: "faux", provider: "faux", model: "faux-1" },
        },
        {
          role: "tool",
          result: {
            call_id: "read-1",
            name: "read_file",
            content: [{ type: "text", text: "value\n" }],
            details: { path: "value.txt" },
            is_error: false,
          },
        },
      ],
      tools: [],
    },
    {
      model: faux.getModel(),
      streamFn: faux.provider.streamSimple.bind(faux.provider),
    },
  );

  assert.equal(response.stop_reason, "stop");
  assert.deepEqual(response.content, [
    { type: "reasoning", text: "checked", redacted: false },
    { type: "text", text: "Done." },
  ]);
});

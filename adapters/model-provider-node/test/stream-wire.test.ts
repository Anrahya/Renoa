import assert from "node:assert/strict";
import { test } from "node:test";

import {
  anthropicSse,
  oauthCredential,
  responsesSse,
  startFakeServer,
  successfulChat,
  tempDir,
} from "./helpers.js";
import {
  runStream,
  withOpenCode,
} from "./stream-support.js";

test("xAI Grok 4.6 sends distinct reasoning_effort values on the chat completions wire", async () => {
  const efforts = ["low", "high", "xhigh"] as const;
  const bodies: string[] = [];
  for (const effort of efforts) {
    const server = await startFakeServer();
    server.enqueue({ sse: successfulChat(effort) });
    const directory = tempDir();
    try {
      await runStream({
        directory: directory.path,
        modelId: "grok-4.6",
        baseUrl: server.baseUrl,
        credential: oauthCredential(),
        reasoningLevel: effort,
      });
      assert.equal(server.requests.length, 1);
      const request = server.requests[0];
      assert.ok(request);
      assert.equal(request.method, "POST");
      assert.equal(request.url, "/v1/chat/completions");
      assert.match(request.headers.authorization ?? "", /^Bearer /);
      const body = JSON.parse(request.body) as {
        model?: string;
        reasoning_effort?: string;
        max_completion_tokens?: number;
        stream?: boolean;
      };
      assert.equal(body.model, "grok-4.6");
      assert.equal(body.reasoning_effort, effort);
      assert.equal(body.stream, true);
      assert.equal(typeof body.max_completion_tokens, "number");
      bodies.push(request.body);
    } finally {
      await server.close();
      directory.close();
    }
  }
  assert.equal(new Set(bodies).size, 3);
});

test("OpenCode Go Ox Alpha sends its documented chat, reasoning, and tool fields", async () => {
  const server = await startFakeServer();
  server.enqueue({ sse: successfulChat("ox") });
  const directory = tempDir();
  try {
    await runStream({
      directory: directory.path,
      provider: "opencode-go",
      modelId: "ox-alpha-free",
      baseUrl: server.baseUrl,
      credential: { type: "api_key", key: "opencode-test-key" },
      reasoningLevel: "max",
    });
    const request = server.requests[0];
    assert.ok(request);
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/v1/chat/completions");
    assert.match(request.headers.authorization ?? "", /^Bearer opencode-test-key$/);
    const body = JSON.parse(request.body) as {
      model?: string;
      reasoning_effort?: string;
      max_tokens?: number;
      max_completion_tokens?: number;
      tools?: unknown[];
    };
    assert.equal(body.model, "ox-alpha-free");
    assert.equal(body.reasoning_effort, "max");
    assert.equal(typeof body.max_tokens, "number");
    assert.equal(body.max_completion_tokens, undefined);
    assert.equal(body.tools?.length, 1);
  } finally {
    await server.close();
    directory.close();
  }
});

test("each supported transport uses the official method, route, auth, and body fields", async () => {
  const chat = await startFakeServer();
  chat.enqueue({ sse: successfulChat("chat") });
  const chatDir = tempDir();
  try {
    await runStream({
      directory: chatDir.path,
      modelId: "grok-4.6",
      baseUrl: chat.baseUrl,
      credential: oauthCredential(),
      reasoningLevel: "high",
    });
    const request = chat.requests[0];
    assert.ok(request);
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/v1/chat/completions");
    assert.match(request.headers.authorization ?? "", /^Bearer access-token-old$/);
    const chatBody = JSON.parse(request.body) as {
      model?: string;
      reasoning_effort?: string;
      messages?: unknown[];
    };
    assert.equal(chatBody.model, "grok-4.6");
    assert.equal(chatBody.reasoning_effort, "high");
    assert.ok(Array.isArray(chatBody.messages));
  } finally {
    await chat.close();
    chatDir.close();
  }

  await withOpenCode(
    "glm-5.1",
    (server) => {
      server.enqueue({ sse: successfulChat("glm") });
      return server.baseUrl;
    },
    (_records, server) => {
      const request = server.requests[0];
      assert.ok(request);
      assert.equal(request.method, "POST");
      assert.equal(request.url, "/v1/chat/completions");
      assert.match(request.headers.authorization ?? "", /^Bearer opencode-test-key$/);
      const body = JSON.parse(request.body) as {
        model?: string;
        max_tokens?: number;
        max_completion_tokens?: number;
      };
      assert.equal(body.model, "glm-5.1");
      assert.equal(typeof body.max_tokens, "number");
      assert.equal(body.max_completion_tokens, undefined);
    },
  );

  await withOpenCode(
    "grok-4.5",
    (server) => {
      server.enqueue({ sse: responsesSse("responses") });
      return server.baseUrl;
    },
    (_records, server) => {
      const request = server.requests[0];
      assert.ok(request);
      assert.equal(request.method, "POST");
      assert.equal(request.url, "/v1/responses");
      assert.match(request.headers.authorization ?? "", /^Bearer opencode-test-key$/);
      const body = JSON.parse(request.body) as {
        model?: string;
        reasoning?: { effort?: string };
        stream?: boolean;
      };
      assert.equal(body.model, "grok-4.5");
      assert.equal(body.stream, true);
      assert.equal(typeof body.reasoning?.effort, "string");
    },
  );

  await withOpenCode(
    "minimax-m3",
    (server) => {
      server.enqueue({ sse: anthropicSse("anthropic") });
      return server.origin;
    },
    (_records, server) => {
      const request = server.requests[0];
      assert.ok(request);
      assert.equal(request.method, "POST");
      assert.equal(request.url, "/v1/messages");
      assert.equal(request.headers["x-api-key"], "opencode-test-key");
      const body = JSON.parse(request.body) as {
        model?: string;
        max_tokens?: number;
        stream?: boolean;
      };
      assert.equal(body.model, "minimax-m3");
      assert.equal(body.stream, true);
      assert.equal(typeof body.max_tokens, "number");
    },
  );
});

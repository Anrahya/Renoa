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

test("xAI Grok 4.6 sends distinct reasoning efforts on the Responses wire", async () => {
  const efforts = ["low", "high", "xhigh"] as const;
  const bodies: string[] = [];
  for (const effort of efforts) {
    const server = await startFakeServer();
    server.enqueue({ sse: responsesSse(effort) });
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
      assert.equal(request.url, "/v1/responses");
      assert.match(request.headers.authorization ?? "", /^Bearer /);
      const body = JSON.parse(request.body) as {
        model?: string;
        reasoning?: { effort?: string };
        include?: string[];
        max_output_tokens?: number;
        store?: boolean;
        stream?: boolean;
      };
      assert.equal(body.model, "grok-4.6");
      assert.equal(body.reasoning?.effort, effort);
      assert.deepEqual(body.include, ["reasoning.encrypted_content"]);
      assert.equal(body.store, false);
      assert.equal(body.stream, true);
      assert.equal(typeof body.max_output_tokens, "number");
      bodies.push(request.body);
    } finally {
      await server.close();
      directory.close();
    }
  }
  assert.equal(new Set(bodies).size, 3);
});

test("OpenCode Go GLM-5.3 sends its documented chat, reasoning, and tool fields", async () => {
  const server = await startFakeServer();
  server.enqueue({ sse: successfulChat("glm") });
  const directory = tempDir();
  try {
    await runStream({
      directory: directory.path,
      provider: "opencode-go",
      modelId: "glm-5.3",
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
    assert.equal(body.model, "glm-5.3");
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
  const xai = await startFakeServer();
  xai.enqueue({ sse: responsesSse("xai") });
  const xaiDir = tempDir();
  try {
    await runStream({
      directory: xaiDir.path,
      modelId: "grok-4.6",
      baseUrl: xai.baseUrl,
      credential: oauthCredential(),
      reasoningLevel: "high",
    });
    const request = xai.requests[0];
    assert.ok(request);
    assert.equal(request.method, "POST");
    assert.equal(request.url, "/v1/responses");
    assert.match(request.headers.authorization ?? "", /^Bearer access-token-old$/);
    const xaiBody = JSON.parse(request.body) as {
      model?: string;
      reasoning?: { effort?: string };
      input?: unknown[];
    };
    assert.equal(xaiBody.model, "grok-4.6");
    assert.equal(xaiBody.reasoning?.effort, "high");
    assert.ok(Array.isArray(xaiBody.input));
  } finally {
    await xai.close();
    xaiDir.close();
  }

  await withOpenCode(
    "glm-5.3",
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
      assert.equal(body.model, "glm-5.3");
      assert.equal(typeof body.max_tokens, "number");
      assert.equal(body.max_completion_tokens, undefined);
    },
  );

  await withOpenCode(
    "grok-4.7",
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
      assert.equal(body.model, "grok-4.7");
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

import assert from "node:assert/strict";
import { test } from "node:test";

import { oauthCredential, responsesSse, startFakeServer, tempDir } from "./helpers.js";
import { runStream } from "./stream-support.js";

const SESSION_ID = "11111111-1111-4111-8111-111111111111";

// No live xAI call is possible in CI; this pins the Responses body the
// adapter sends today for the pinned grok-4.3 catalog entry.
test("xAI Responses requests carry the developer prompt, encrypted reasoning, and the session cache key", async () => {
  const server = await startFakeServer();
  server.enqueue({ sse: responsesSse("high") });
  server.enqueue({ sse: responsesSse("off") });
  const directory = tempDir();
  try {
    for (const reasoningLevel of ["high", "off"] as const) {
      await runStream({
        directory: directory.path,
        provider: "xai",
        modelId: "grok-4.3",
        baseUrl: server.origin,
        credential: oauthCredential(),
        reasoningLevel,
      });
    }
    const [high, off] = server.requests;
    assert.ok(high && off);
    assert.equal(high.method, "POST");
    assert.equal(high.url, "/responses");
    const body = JSON.parse(high.body) as Record<string, unknown>;
    assert.equal(body.model, "grok-4.3");
    assert.equal(body.store, false);
    assert.equal(body.prompt_cache_key, SESSION_ID);
    assert.equal(body.prompt_cache_retention, undefined);
    assert.equal(body.max_output_tokens, 128);
    assert.deepEqual((body.input as { role: string }[])[0], {
      role: "developer",
      content: "You are a test model.",
    });
    assert.deepEqual(body.reasoning, { effort: "high", summary: "auto" });
    assert.deepEqual(body.include, ["reasoning.encrypted_content"]);
    assert.deepEqual(
      (body.tools as { type: string; name: string; strict?: unknown }[]).map((tool) => [tool.type, tool.name, tool.strict]),
      [["function", "lookup", undefined]],
    );

    // grok-4.3 maps off to effort "none"; xAI still asks for encrypted reasoning.
    const offBody = JSON.parse(off.body) as Record<string, unknown>;
    assert.deepEqual(offBody.reasoning, { effort: "none" });
    assert.deepEqual(offBody.include, ["reasoning.encrypted_content"]);
    assert.equal((offBody.input as { role: string }[])[0]?.role, "developer");
  } finally {
    await server.close();
    directory.close();
  }
});

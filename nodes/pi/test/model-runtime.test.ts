import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { SqliteCredentialStore } from "../src/credentials.js";
import { loadModelRuntime } from "../src/model-runtime.js";

test("the xAI runtime resolves a model from a durable OAuth login", async () => {
  const directory = await mkdtemp(join(tmpdir(), "renoa-pi-xai-runtime-"));
  const authStorePath = join(directory, "auth.sqlite");
  try {
    const credentials = new SqliteCredentialStore(authStorePath);
    await credentials.modify("xai", async () => ({
      type: "oauth",
      access: "access-token",
      refresh: "refresh-token",
      expires: 2_000_000_000_000,
    }));
    credentials.close();

    const runtime = await loadModelRuntime({
      provider: "xai",
      modelId: "grok-4.5",
      authStorePath,
      catalogBaseUrl: "http://127.0.0.1:1",
    });

    assert.equal(runtime.model.provider, "xai");
    assert.equal(runtime.model.id, "grok-4.5");
    runtime.close();
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

test("the xAI runtime refuses to start before login", async () => {
  const directory = await mkdtemp(join(tmpdir(), "renoa-pi-xai-unconfigured-"));
  try {
    await assert.rejects(
      loadModelRuntime({
        provider: "xai",
        modelId: "grok-4.6",
        authStorePath: join(directory, "auth.sqlite"),
        catalogBaseUrl: "http://127.0.0.1:1",
      }),
      /xai credentials are not configured/,
    );
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

test("the xAI runtime resolves a model added to Pi's live catalog", async () => {
  const directory = await mkdtemp(join(tmpdir(), "renoa-pi-xai-catalog-"));
  const authStorePath = join(directory, "auth.sqlite");
  let catalogRequests = 0;
  const server = createServer((request, response) => {
    catalogRequests += 1;
    assert.equal(request.url, "/api/models/providers/xai");
    if (catalogRequests === 1) {
      response.writeHead(503).end();
      return;
    }
    response.writeHead(200, { "content-type": "application/json" });
    response.end(
      JSON.stringify({
        "grok-4.6": {
          id: "grok-4.6",
          name: "Grok 4.6",
          api: "openai-completions",
          provider: "xai",
          baseUrl: "https://api.x.ai/v1",
          reasoning: true,
          input: ["text", "image"],
          cost: { input: 2, output: 6, cacheRead: 0.5, cacheWrite: 0 },
          contextWindow: 500_000,
          maxTokens: 500_000,
          compat: {
            supportsStore: false,
            supportsDeveloperRole: false,
            supportsReasoningEffort: false,
          },
        },
        "grok-evil": {
          id: "grok-evil",
          name: "Grok Evil",
          api: "openai-completions",
          provider: "xai",
          baseUrl: "https://evil.invalid/v1",
          reasoning: true,
          input: ["text"],
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
          contextWindow: 1,
          maxTokens: 1,
        },
      }),
    );
  });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  assert.ok(address !== null && typeof address !== "string");
  try {
    const credentials = new SqliteCredentialStore(authStorePath);
    await credentials.modify("xai", async () => ({
      type: "oauth",
      access: "access-token",
      refresh: "refresh-token",
      expires: 2_000_000_000_000,
    }));
    credentials.close();

    const runtime = await loadModelRuntime({
      provider: "xai",
      modelId: "grok-4.6",
      authStorePath,
      catalogBaseUrl: `http://127.0.0.1:${address.port}`,
    });

    assert.equal(runtime.model.id, "grok-4.6");
    assert.equal(runtime.model.api, "openai-completions");
    assert.match(runtime.modelBindingId, /^[0-9a-f]{64}$/u);
    const pinnedSpec = JSON.parse(runtime.modelSpec) as unknown;
    const bindingId = runtime.modelBindingId;
    runtime.close();

    const pinned = await loadModelRuntime({
      provider: "xai",
      modelId: "grok-4.6",
      authStorePath,
      catalogBaseUrl: `http://127.0.0.1:${address.port}`,
      modelSpec: pinnedSpec,
    });
    assert.equal(pinned.modelBindingId, bindingId);
    pinned.close();
    assert.equal(catalogRequests, 2, "a pinned model must not re-read the live catalog");

    await assert.rejects(
      loadModelRuntime({
        provider: "xai",
        modelId: "grok-evil",
        authStorePath,
        catalogBaseUrl: `http://127.0.0.1:${address.port}`,
      }),
      /model binding .* is invalid/u,
    );
    assert.equal(catalogRequests, 3);
  } finally {
    await new Promise<void>((resolve, reject) => {
      server.close((error) => (error === undefined ? resolve() : reject(error)));
    });
    await rm(directory, { force: true, recursive: true });
  }
});

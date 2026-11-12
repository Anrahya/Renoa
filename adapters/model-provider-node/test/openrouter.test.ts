import assert from "node:assert/strict";
import { join } from "node:path";
import { test } from "node:test";

import { findCatalogModel, validateModelSpec } from "../src/catalog.js";
import type { CatalogModel } from "../src/contract.js";
import { loadCatalog, loadRuntime } from "../src/runtime.js";
import { streamModel } from "../src/stream.js";
import { createStore, startFakeServer, successfulChat, tempDir, userRequest } from "./helpers.js";

const KEY = { type: "api_key", key: "openrouter-test-key" } as const;
const ERNIE = "baidu/ernie-5.5-flash-preview:free";
const PINNED = "~moonshotai/kimi-latest";

/** One entry of OpenRouter's `GET /api/v1/models`, as it listed ERNIE on 2026-11-12. */
function ernie(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: ERNIE,
    name: "Baidu: ERNIE-5.5-Flash-Preview (free)",
    context_length: 1_024_000,
    architecture: { input_modalities: ["text"], output_modalities: ["text"] },
    pricing: { prompt: "0", completion: "0" },
    top_provider: { context_length: 1_024_000, max_completion_tokens: 921_600 },
    supported_parameters: ["include_reasoning", "max_tokens", "reasoning", "reasoning_effort", "tools"],
    reasoning: { mandatory: true, default_enabled: true, supported_efforts: ["high"], default_effort: "high" },
    ...overrides,
  };
}

function listing(...models: Record<string, unknown>[]): typeof globalThis.fetch {
  return async (input) => {
    assert.equal(String(input), "https://openrouter.ai/api/v1/models");
    return new Response(JSON.stringify({ data: models }), { status: 200 });
  };
}

const unreachable: typeof globalThis.fetch = async () => {
  throw new Error("offline");
};

async function catalog(
  directory: string,
  fetch: typeof globalThis.fetch,
  warnings: string[] = [],
): Promise<readonly CatalogModel[]> {
  return loadCatalog("openrouter", join(directory, "credentials.sqlite"), {
    openRouter: {
      cachePath: join(directory, "openrouter-catalog.json"),
      fetch,
      warn: (message) => warnings.push(message),
    },
  });
}

test("OpenRouter learns a tool-capable model Pi does not pin, at the efforts it lists", async () => {
  const directory = tempDir();
  try {
    createStore(directory.path, KEY, "openrouter").close();
    const noTools = ernie({ id: "acme/chat-only", supported_parameters: ["max_tokens"] });
    const models = await catalog(directory.path, listing(ernie(), noTools, { ...ernie(), id: PINNED }));
    const learned = models.find((model) => model.id === ERNIE);
    assert.ok(learned);
    assert.deepEqual(learned.reasoning_levels, ["high"]);
    assert.equal(learned.context_window_tokens, 1_024_000);
    const spec = learned.model_spec as Record<string, unknown>;
    assert.equal(spec.baseUrl, "https://openrouter.ai/api/v1");
    assert.equal(spec.api, "openai-completions");
    assert.equal(spec.maxTokens, 921_600);
    assert.deepEqual(spec.cost, { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 });
    assert.equal(spec.compat, undefined);
    assert.equal(models.some((model) => model.id === "acme/chat-only"), false);
    // A pinned model keeps Pi's binding over the live listing.
    const pinned = models.find((model) => model.id === PINNED);
    assert.deepEqual(pinned?.model_spec, findCatalogModel("openrouter", PINNED)?.model);
  } finally {
    directory.close();
  }
});

test("a learned OpenRouter model keeps its binding and stays listed while OpenRouter is down", async () => {
  const directory = tempDir();
  try {
    createStore(directory.path, KEY, "openrouter").close();
    const first = await catalog(directory.path, listing(ernie()));
    const repriced = await catalog(
      directory.path,
      listing(ernie({ pricing: { prompt: "0.000001", completion: "0.000002" } })),
    );
    const binding = (models: readonly CatalogModel[]) =>
      models.find((model) => model.id === ERNIE)?.model_spec;
    assert.ok(binding(first));
    assert.deepEqual(binding(repriced), binding(first));

    const warnings: string[] = [];
    const offline = await catalog(directory.path, unreachable, warnings);
    assert.deepEqual(binding(offline), binding(first));
    assert.match(warnings.join("\n"), /OpenRouter model list refresh failed: offline/u);

    const retired = await catalog(directory.path, listing(ernie({ id: "baidu/ernie-6" })));
    assert.equal(binding(retired), undefined);
  } finally {
    directory.close();
  }
});

test("OpenRouter falls back to Pi's catalog when it has never been reached", async () => {
  const directory = tempDir();
  try {
    createStore(directory.path, KEY, "openrouter").close();
    const models = await catalog(directory.path, unreachable);
    assert.ok(models.some((model) => model.id === PINNED));
    assert.equal(models.some((model) => model.id === ERNIE), false);
  } finally {
    directory.close();
  }
});

test("an OpenRouter binding must point at OpenRouter", () => {
  const pinned = findCatalogModel("openrouter", PINNED);
  assert.ok(pinned);
  assert.throws(
    () => validateModelSpec({ ...pinned.model, baseUrl: "https://example.com/api/v1" }, "openrouter", PINNED),
    /invalid/u,
  );
});

test("ERNIE streams with the key and OpenRouter's reasoning effort", async () => {
  const directory = tempDir();
  const server = await startFakeServer();
  server.enqueue({ sse: successfulChat("hi") });
  try {
    createStore(directory.path, KEY, "openrouter").close();
    const learned = (await catalog(directory.path, listing(ernie()))).find((model) => model.id === ERNIE);
    assert.ok(learned);
    const runtime = loadRuntime({
      provider: "openrouter",
      modelId: ERNIE,
      authStorePath: join(directory.path, "credentials.sqlite"),
      modelSpec: { ...(learned.model_spec as object), baseUrl: server.origin },
      allowLoopback: true,
    });
    let completed = false;
    try {
      await streamModel({
        runtime,
        request: userRequest(),
        maxOutputTokens: 128,
        signal: new AbortController().signal,
        emit: (record) => {
          completed ||= record.event === "completed";
        },
      });
    } finally {
      runtime.close();
    }
    assert.ok(completed);
    const request = server.requests[0];
    assert.ok(request);
    assert.equal(request.url, "/chat/completions");
    assert.equal(request.headers.authorization, "Bearer openrouter-test-key");
    const body = JSON.parse(request.body) as { model?: string; reasoning?: unknown; tools?: unknown[] };
    assert.equal(body.model, ERNIE);
    assert.deepEqual(body.reasoning, { effort: "high" });
    assert.equal(body.tools?.length, 1);
  } finally {
    await server.close();
    directory.close();
  }
});

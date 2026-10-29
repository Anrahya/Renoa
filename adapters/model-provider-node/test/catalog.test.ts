import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";

import {
  findCatalogModel,
  isAdvertisableCatalogEntry,
  loadPinnedCatalog,
  modelBindingId,
  toWireCatalog,
  validateModelSpec,
} from "../src/catalog.js";
import { loadCatalog } from "../src/runtime.js";
import {
  OPENCODE_GO_TRANSPORTS,
  opencodeGoTransport,
} from "../src/providers/opencode-go.js";
import { oauthCredential, tempDir } from "./helpers.js";
import { SqliteCredentialStore } from "../src/credentials.js";

test("xAI catalog binding ids match SHA-256 of the advertised model spec JSON", () => {
  const entries = loadPinnedCatalog("xai");
  assert.equal(new Set(entries.map((entry) => entry.id)).size, entries.length);
  for (const entry of entries) {
    assert.equal(modelBindingId(entry.model), sha256(JSON.stringify(entry.model)));
    assert.equal(entry.model.provider, "xai");
    assert.ok(entry.reasoning_levels.length > 0);
  }
});

test("OpenCode catalog uses official transports and does not guess from the model name", () => {
  const advertised = new Map(loadPinnedCatalog("opencode-go").map((entry) => [entry.id, entry]));
  // Bundled Pi entries absent from OpenCode's documented model table are not advertised.
  assert.equal(advertised.has("longcat-2.5-preview-free"), false);
  assert.equal(advertised.has("minimax-m3.1-flash-free"), false);

  const minimax = advertised.get("minimax-m3");
  const haiku = advertised.get("claude-haiku-5-5");
  assert.equal(minimax?.model.api, "anthropic-messages");
  assert.equal(minimax?.model.baseUrl, "https://opencode.ai/inference/go");
  assert.equal(haiku?.model.api, "anthropic-messages");
  assert.equal(haiku?.model.baseUrl, "https://opencode.ai/inference/go");

  for (const [id, entry] of advertised) {
    assert.equal(entry.model.api, OPENCODE_GO_TRANSPORTS[id]);
  }

  const glm = advertised.get("glm-5.3");
  assert.equal(glm?.model.api, "openai-completions");
  assert.equal(glm?.model.baseUrl, "https://opencode.ai/inference/go/v1");
  assert.equal(modelBindingId(glm!.model), sha256(JSON.stringify(glm!.model)));
});

test("Muse Spark 1.3 uses the documented Responses transport", () => {
  assert.equal(
    opencodeGoTransport("muse-spark-1.3-contributor", "@ai-sdk/openai-compatible"),
    "openai-responses",
  );
});

test("only chat models on a retained API are advertised", () => {
  const chat = { id: "chat-model", api: "openai-completions", type: "chat" };
  assert.equal(isAdvertisableCatalogEntry("opencode-go", chat), true);
  assert.equal(isAdvertisableCatalogEntry("opencode-go", { ...chat, type: undefined }), true);
  assert.equal(isAdvertisableCatalogEntry("opencode-go", { ...chat, type: "image" }), false);
  assert.equal(isAdvertisableCatalogEntry("opencode-go", { ...chat, type: "classifier" }), false);
  assert.equal(
    isAdvertisableCatalogEntry("opencode-go", { ...chat, api: "google-generative-ai" }),
    false,
  );
  assert.equal(isAdvertisableCatalogEntry("xai", { ...chat, api: "anthropic-messages" }), false);
  // xAI is served through the Responses API only.
  assert.equal(isAdvertisableCatalogEntry("xai", { ...chat, api: "openai-completions" }), false);
  assert.equal(isAdvertisableCatalogEntry("xai", { ...chat, api: "openai-responses" }), true);

  for (const provider of ["xai", "opencode-go"] as const) {
    for (const entry of loadPinnedCatalog(provider)) {
      assert.notEqual(entry.model.api, "google-generative-ai");
      assert.equal("type" in entry.model, false, `${entry.id} must not carry catalog type`);
      assert.equal("inputLimits" in entry.model, false, `${entry.id} must not carry input limits`);
    }
  }
});

test("OpenCode Go bindings use the inference endpoints and refuse the retired zen route", () => {
  const glm = findCatalogModel("opencode-go", "glm-5.3");
  assert.ok(glm);
  assert.throws(
    () =>
      validateModelSpec(
        { ...glm.model, baseUrl: "https://opencode.ai/zen/go/v1" },
        "opencode-go",
        "glm-5.3",
      ),
    /invalid/,
  );
});

test("wire catalog exposes each validated model context window", () => {
  const entries = loadPinnedCatalog("opencode-go");
  const wire = toWireCatalog(entries);
  assert.equal(wire.length, entries.length);
  for (const [index, entry] of entries.entries()) {
    assert.equal(wire[index]?.context_window_tokens, entry.model.contextWindow);
  }
});

test("catalog probing is read-only and does not rewrite stored credentials", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  try {
    const store = new SqliteCredentialStore(path);
    store.write("xai", oauthCredential());
    const before = store.read("xai");
    store.close();
    const models = await loadCatalog("xai", path);
    assert.ok(models.some((model) => model.id === "grok-4.6"));
    const after = new SqliteCredentialStore(path);
    assert.deepEqual(after.read("xai"), before);
    after.close();
  } finally {
    directory.close();
  }
});

test("xAI catalog rejects API-key credentials", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  try {
    const store = new SqliteCredentialStore(path);
    store.write("xai", { type: "api_key", key: "xai-api-key" });
    store.close();
    await assert.rejects(loadCatalog("xai", path), /xAI requires OAuth credentials/u);
  } finally {
    directory.close();
  }
});

test("OpenCode Go catalog rejects OAuth credentials", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  try {
    const store = new SqliteCredentialStore(path);
    store.write("opencode-go", oauthCredential());
    store.close();
    await assert.rejects(
      loadCatalog("opencode-go", path),
      /OpenCode Go requires an API key/u,
    );
  } finally {
    directory.close();
  }
});

test("OpenCode Go discovers a new official model and reuses cached metadata on 304", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  const cachePath = join(directory.path, "catalog.json");
  try {
    writeOpenCodeCredential(path);
    let generation = 0;
    const requests: Array<{ url: string; etag: string | null }> = [];
    const fetcher: typeof globalThis.fetch = async (input, init) => {
      const url = requestUrl(input);
      const etag = new Headers(init?.headers).get("if-none-match");
      requests.push({ url, etag });
      if (url.endsWith("/models")) {
        const ids = generation === 0 ? ["glm-5.3"] : ["glm-5.3", LIVE_MODEL];
        return jsonResponse({ data: ids.map((id) => ({ id })) });
      }
      if (etag === '"catalog-1"') {
        return new Response(null, { status: 304 });
      }
      return jsonResponse(modelsDevCatalog(flashMetadata()), { etag: '"catalog-1"' });
    };

    const first = await loadCatalog("opencode-go", path, {
      openCode: { cachePath, fetch: fetcher, warn: () => undefined },
    });
    assert.equal(first.some((model) => model.id === LIVE_MODEL), false);
    assert.equal(
      first.some((model) => model.id === "mimo-v2.5"),
      true,
      "the public inventory must not remove bundled catalog entries",
    );

    generation = 1;
    const second = await loadCatalog("opencode-go", path, {
      openCode: { cachePath, fetch: fetcher, warn: () => undefined },
    });
    const live = second.find((model) => model.id === LIVE_MODEL);
    assert.ok(live);
    const modelSpec = live.model_spec as Record<string, unknown>;
    assert.equal(live.name, "Kimi K3.1 Pro");
    assert.equal(live.context_window_tokens, 1_000_000);
    assert.equal(modelSpec.api, "openai-completions");
    assert.equal(modelSpec.baseUrl, "https://opencode.ai/inference/go/v1");
    assert.deepEqual(modelSpec.input, ["text", "image"]);
    assert.deepEqual(live.reasoning_levels, ["low", "high", "max"]);
    assert.equal(
      requests.some((request) => request.url === "https://models.dev/api.json" && request.etag === '"catalog-1"'),
      true,
    );
  } finally {
    directory.close();
  }
});

test("OpenCode Go keeps a learned binding stable and falls back to it offline", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  const cachePath = join(directory.path, "catalog.json");
  try {
    writeOpenCodeCredential(path);
    let contextWindow = 1_000_000;
    const liveFetch: typeof globalThis.fetch = async (input) => {
      const url = requestUrl(input);
      return url.endsWith("/models")
        ? jsonResponse({ data: [{ id: LIVE_MODEL }] })
        : jsonResponse(modelsDevCatalog(flashMetadata(contextWindow)));
    };
    const first = await loadCatalog("opencode-go", path, {
      openCode: { cachePath, fetch: liveFetch, warn: () => undefined },
    });
    const firstBinding = first.find((model) => model.id === LIVE_MODEL)?.model_spec;
    assert.ok(firstBinding);

    contextWindow = 123_456;
    const refreshed = await loadCatalog("opencode-go", path, {
      openCode: { cachePath, fetch: liveFetch, warn: () => undefined },
    });
    assert.deepEqual(
      refreshed.find((model) => model.id === LIVE_MODEL)?.model_spec,
      firstBinding,
      "an automatic metadata refresh must not mutate a learned runtime binding",
    );

    const warnings: string[] = [];
    const offline = await loadCatalog("opencode-go", path, {
      openCode: {
        cachePath,
        fetch: async () => {
          throw new Error("offline");
        },
        warn: (warning) => warnings.push(warning),
      },
    });
    assert.deepEqual(
      offline.find((model) => model.id === LIVE_MODEL)?.model_spec,
      firstBinding,
    );
    assert.equal(warnings.some((warning) => warning.includes("availability refresh failed")), true);
  } finally {
    directory.close();
  }
});

test("OpenCode Go replaces a cached binding learned on the retired zen route", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  const cachePath = join(directory.path, "catalog.json");
  try {
    writeOpenCodeCredential(path);
    const retired = {
      id: LIVE_MODEL,
      name: "Kimi K3.1 Pro",
      api: "openai-completions",
      provider: "opencode-go",
      baseUrl: "https://opencode.ai/zen/go/v1",
      reasoning: true,
      input: ["text"],
      cost: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0 },
      contextWindow: 1_000_000,
      maxTokens: 131_072,
    };
    writeFileSync(
      cachePath,
      JSON.stringify({ version: 1, available_model_ids: [LIVE_MODEL], dynamic_models: [retired] }),
    );
    const models = await loadCatalog("opencode-go", path, {
      openCode: {
        cachePath,
        fetch: async (input) =>
          requestUrl(input).endsWith("/models")
            ? jsonResponse({ data: [{ id: LIVE_MODEL }] })
            : jsonResponse(modelsDevCatalog(flashMetadata())),
        warn: () => undefined,
      },
    });
    const live = models.find((model) => model.id === LIVE_MODEL);
    assert.equal((live?.model_spec as Record<string, unknown>).baseUrl, "https://opencode.ai/inference/go/v1");
    const cached = JSON.parse(readFileSync(cachePath, "utf8")) as { dynamic_models: { baseUrl: string }[] };
    assert.deepEqual(
      cached.dynamic_models.map((model) => model.baseUrl),
      ["https://opencode.ai/inference/go/v1"],
    );
  } finally {
    directory.close();
  }
});

test("OpenCode Go rejects an oversized metadata response", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  const cachePath = join(directory.path, "catalog.json");
  try {
    writeOpenCodeCredential(path);
    const warnings: string[] = [];
    const models = await loadCatalog("opencode-go", path, {
      openCode: {
        cachePath,
        fetch: async (input) =>
          requestUrl(input).endsWith("/models")
            ? jsonResponse({ data: [{ id: LIVE_MODEL }] })
            : new Response("{}", {
                status: 200,
                headers: { "content-length": String(8 * 1024 * 1024 + 1) },
              }),
        warn: (warning) => warnings.push(warning),
      },
    });

    assert.equal(models.some((model) => model.id === LIVE_MODEL), false);
    assert.equal(warnings.some((warning) => warning.includes("byte limit")), true);
  } finally {
    directory.close();
  }
});

test("Grok 4.6 advertises verified reasoning levels and no off or minimal", () => {
  const grok = findCatalogModel("xai", "grok-4.6");
  assert.ok(grok);
  assert.deepEqual([...grok.reasoning_levels], ["low", "medium", "high", "xhigh"]);
});

test("xAI models use the Responses API and Grok 4.3 offers off through xhigh", () => {
  for (const entry of loadPinnedCatalog("xai")) {
    assert.equal(entry.model.api, "openai-responses", entry.id);
  }
  const grok = findCatalogModel("xai", "grok-4.3");
  assert.ok(grok);
  assert.deepEqual([...grok.reasoning_levels], ["off", "low", "medium", "high", "xhigh"]);
});

test("model specs reject untrusted base URLs unless loopback tests are enabled", () => {
  const grok = findCatalogModel("xai", "grok-4.6");
  assert.ok(grok);
  assert.throws(
    () =>
      validateModelSpec(
        { ...grok.model, baseUrl: "http://127.0.0.1:9/v1" },
        "xai",
        "grok-4.6",
      ),
    /invalid/,
  );
  const loopback = validateModelSpec(
    { ...grok.model, baseUrl: "http://127.0.0.1:9/v1" },
    "xai",
    "grok-4.6",
    { allowLoopback: true },
  );
  assert.equal(loopback.baseUrl, "http://127.0.0.1:9/v1");
});

/** A documented OpenCode Go model that only live discovery supplies. */
const LIVE_MODEL = "kimi-k3.1-pro";

function sha256(value: string): string {
  return createHash("sha256").update(value).digest("hex");
}

function writeOpenCodeCredential(path: string): void {
  const store = new SqliteCredentialStore(path);
  store.write("opencode-go", { type: "api_key", key: "test-key" });
  store.close();
}

function requestUrl(input: Parameters<typeof globalThis.fetch>[0]): string {
  return typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
}

function jsonResponse(
  value: unknown,
  headers: Record<string, string> = {},
): Response {
  return new Response(JSON.stringify(value), {
    status: 200,
    headers: { "content-type": "application/json", ...headers },
  });
}

function modelsDevCatalog(model: Record<string, unknown>): Record<string, unknown> {
  return {
    "opencode-go": {
      id: "opencode-go",
      npm: "@ai-sdk/openai-compatible",
      models: { [LIVE_MODEL]: model },
    },
  };
}

function flashMetadata(context = 1_000_000): Record<string, unknown> {
  return {
    id: LIVE_MODEL,
    name: "Kimi K3.1 Pro",
    attachment: true,
    reasoning: true,
    reasoning_options: [{ type: "effort", values: ["low", "high", "max"] }],
    tool_call: true,
    interleaved: { field: "reasoning_content" },
    structured_output: true,
    modalities: { input: ["text", "image", "video", "pdf"], output: ["text"] },
    limit: { context, output: 131_072 },
    cost: { input: 0.075, output: 0.25, cache_read: 0.015 },
  };
}

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { join } from "node:path";
import { test } from "node:test";

import {
  findCatalogModel,
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
  for (const entry of loadPinnedCatalog("xai")) {
    assert.equal(modelBindingId(entry.model), sha256(JSON.stringify(entry.model)));
    assert.equal(entry.model.provider, "xai");
    assert.ok(entry.reasoning_levels.length > 0);
  }
});

test("OpenCode catalog uses official transports and does not guess from the model name", () => {
  const advertised = new Map(loadPinnedCatalog("opencode-go").map((entry) => [entry.id, entry]));
  assert.equal(advertised.has("muse-spark-1.2-contributor"), false);
  assert.equal(advertised.has("minimax-m2.5"), false);

  const minimax = advertised.get("minimax-m2.7");
  const qwen = advertised.get("qwen3.6-plus");
  assert.equal(minimax?.model.api, "anthropic-messages");
  assert.equal(minimax?.model.baseUrl, "https://opencode.ai/zen/go");
  assert.equal(qwen?.model.api, "anthropic-messages");
  assert.equal(qwen?.model.baseUrl, "https://opencode.ai/zen/go");

  for (const [id, entry] of advertised) {
    assert.equal(entry.model.api, OPENCODE_GO_TRANSPORTS[id]);
  }

  const glm = advertised.get("glm-5.1");
  assert.equal(glm?.model.api, "openai-completions");
  assert.equal(modelBindingId(glm!.model), sha256(JSON.stringify(glm!.model)));
});

test("Muse Spark 1.3 uses the documented Responses transport", () => {
  assert.equal(
    opencodeGoTransport("muse-spark-1.3-contributor", "@ai-sdk/openai-compatible"),
    "openai-responses",
  );
});

test("Ox Alpha projects the current models.dev contract onto Renoa's supported modalities", () => {
  const ox = findCatalogModel("opencode-go", "ox-alpha-free");
  assert.ok(ox);
  assert.equal(ox.name, "Ox Alpha Free (Unlimited)");
  assert.equal(ox.model.api, "openai-completions");
  assert.equal(ox.model.baseUrl, "https://opencode.ai/zen/go/v1");
  assert.equal(ox.model.contextWindow, 1_000_000);
  assert.equal(ox.model.maxTokens, 131_072);
  assert.deepEqual(ox.model.input, ["text", "image"]);
  assert.deepEqual([...ox.reasoning_levels], ["low", "high", "max"]);
  assert.deepEqual(ox.model.cost, { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 });
});

test("wire catalog exposes each validated model context window", () => {
  const entries = loadPinnedCatalog("opencode-go");
  const wire = toWireCatalog(entries);
  assert.equal(wire.length, entries.length);
  for (const [index, entry] of entries.entries()) {
    assert.equal(wire[index]?.context_window_tokens, entry.model.contextWindow);
  }
});

test("migrated OpenCode models do not keep the previous Pi completions binding id", () => {
  const previousMinimax = {
    id: "minimax-m2.7",
    name: "MiniMax-M2.7",
    api: "openai-completions",
    provider: "opencode-go",
    baseUrl: "https://opencode.ai/zen/go/v1",
    reasoning: true,
    input: ["text"],
    cost: { input: 0.3, output: 1.2, cacheRead: 0.06, cacheWrite: 0 },
    compat: {
      supportsStore: false,
      supportsDeveloperRole: false,
      maxTokensField: "max_tokens",
    },
    contextWindow: 204800,
    maxTokens: 131072,
  };
  const current = findCatalogModel("opencode-go", "minimax-m2.7");
  assert.ok(current);
  assert.notEqual(modelBindingId(current.model), sha256(JSON.stringify(previousMinimax)));
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
        const ids = generation === 0 ? ["glm-5.3"] : ["glm-5.3", "glm-5.3-flash"];
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
    assert.equal(first.some((model) => model.id === "glm-5.3-flash"), false);
    assert.equal(
      first.some((model) => model.id === "ox-alpha-free"),
      true,
      "the public inventory must not remove Renoa's explicit compatibility entries",
    );

    generation = 1;
    const second = await loadCatalog("opencode-go", path, {
      openCode: { cachePath, fetch: fetcher, warn: () => undefined },
    });
    const flash = second.find((model) => model.id === "glm-5.3-flash");
    assert.ok(flash);
    const modelSpec = flash.model_spec as Record<string, unknown>;
    assert.equal(flash.name, "GLM-5.3-Flash (2x usage)");
    assert.equal(flash.context_window_tokens, 1_000_000);
    assert.equal(modelSpec.api, "openai-completions");
    assert.equal(modelSpec.baseUrl, "https://opencode.ai/zen/go/v1");
    assert.deepEqual(modelSpec.input, ["text", "image"]);
    assert.deepEqual(flash.reasoning_levels, ["low", "high", "max"]);
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
        ? jsonResponse({ data: [{ id: "glm-5.3-flash" }] })
        : jsonResponse(modelsDevCatalog(flashMetadata(contextWindow)));
    };
    const first = await loadCatalog("opencode-go", path, {
      openCode: { cachePath, fetch: liveFetch, warn: () => undefined },
    });
    const firstBinding = first.find((model) => model.id === "glm-5.3-flash")?.model_spec;
    assert.ok(firstBinding);

    contextWindow = 123_456;
    const refreshed = await loadCatalog("opencode-go", path, {
      openCode: { cachePath, fetch: liveFetch, warn: () => undefined },
    });
    assert.deepEqual(
      refreshed.find((model) => model.id === "glm-5.3-flash")?.model_spec,
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
      offline.find((model) => model.id === "glm-5.3-flash")?.model_spec,
      firstBinding,
    );
    assert.equal(warnings.some((warning) => warning.includes("availability refresh failed")), true);
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
            ? jsonResponse({ data: [{ id: "glm-5.3-flash" }] })
            : new Response("{}", {
                status: 200,
                headers: { "content-length": String(8 * 1024 * 1024 + 1) },
              }),
        warn: (warning) => warnings.push(warning),
      },
    });

    assert.equal(models.some((model) => model.id === "glm-5.3-flash"), false);
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
      models: { "glm-5.3-flash": model },
    },
  };
}

function flashMetadata(context = 1_000_000): Record<string, unknown> {
  return {
    id: "glm-5.3-flash",
    name: "GLM-5.3-Flash (2x usage)",
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

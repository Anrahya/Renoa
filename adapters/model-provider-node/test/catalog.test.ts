import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { join } from "node:path";
import { test } from "node:test";

import {
  findCatalogModel,
  loadPinnedCatalog,
  modelBindingId,
  validateModelSpec,
} from "../src/catalog.js";
import { loadCatalog } from "../src/runtime.js";
import { OPENCODE_GO_TRANSPORTS } from "../src/providers/opencode-go.js";
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

test("catalog probing is read-only and does not rewrite stored credentials", () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  try {
    const store = new SqliteCredentialStore(path);
    store.write("xai", oauthCredential());
    const before = store.read("xai");
    store.close();
    const models = loadCatalog("xai", path);
    assert.ok(models.some((model) => model.id === "grok-4.6"));
    const after = new SqliteCredentialStore(path);
    assert.deepEqual(after.read("xai"), before);
    after.close();
  } finally {
    directory.close();
  }
});

test("xAI catalog rejects API-key credentials", () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  try {
    const store = new SqliteCredentialStore(path);
    store.write("xai", { type: "api_key", key: "xai-api-key" });
    store.close();
    assert.throws(() => loadCatalog("xai", path), /xAI requires OAuth credentials/u);
  } finally {
    directory.close();
  }
});

test("OpenCode Go catalog rejects OAuth credentials", () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  try {
    const store = new SqliteCredentialStore(path);
    store.write("opencode-go", oauthCredential());
    store.close();
    assert.throws(() => loadCatalog("opencode-go", path), /OpenCode Go requires an API key/u);
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

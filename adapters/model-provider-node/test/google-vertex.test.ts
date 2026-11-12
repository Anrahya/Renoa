import assert from "node:assert/strict";
import { join } from "node:path";
import { test } from "node:test";

import { findCatalogModel, validateModelSpec } from "../src/catalog.js";
import { ProviderFailure } from "../src/errors.js";
import { routedVertexModel } from "../src/providers/google-vertex.js";
import { loadCatalog, loadRuntime } from "../src/runtime.js";
import { streamModel } from "../src/stream.js";
import type { Credential } from "../src/credentials.js";
import { createStore, startFakeServer, tempDir, userRequest } from "./helpers.js";

const HYDROGEN = "gemini-4-hydrogen";
const KEY: Credential = {
  type: "api_key",
  key: "vertex-test-key",
  env: { GOOGLE_CLOUD_PROJECT: "renoa-test-project", GOOGLE_CLOUD_LOCATION: "global" },
};

test("Vertex advertises Gemini 4 Hydrogen beside Pi's catalog, at low, medium and high", async () => {
  const directory = tempDir();
  try {
    createStore(directory.path, KEY, "google-vertex").close();
    const models = await loadCatalog("google-vertex", join(directory.path, "credentials.sqlite"));
    const hydrogen = models.find((model) => model.id === HYDROGEN);
    assert.ok(hydrogen);
    assert.deepEqual(hydrogen.reasoning_levels, ["low", "medium", "high"]);
    assert.ok(models.some((model) => model.id === "gemini-4-argon"));
    const spec = findCatalogModel("google-vertex", HYDROGEN)?.model;
    assert.ok(spec);
    assert.throws(
      () => validateModelSpec({ ...spec, baseUrl: "https://example.com" }, "google-vertex", HYDROGEN),
      /invalid/u,
    );
  } finally {
    directory.close();
  }
});

test("a Vertex key without its project is refused before any request", async () => {
  const directory = tempDir();
  try {
    createStore(directory.path, { type: "api_key", key: "vertex-test-key" }, "google-vertex").close();
    await assert.rejects(
      loadCatalog("google-vertex", join(directory.path, "credentials.sqlite")),
      /needs the key's Google Cloud project/u,
    );
  } finally {
    directory.close();
  }
});

test("a Vertex request goes to the key's project and location", () => {
  const spec = findCatalogModel("google-vertex", HYDROGEN)?.model;
  assert.ok(spec);
  assert.equal(
    routedVertexModel(spec, KEY).baseUrl,
    "https://aiplatform.googleapis.com/v1/projects/renoa-test-project/locations/global",
  );
  const regional: Credential = {
    type: "api_key",
    key: "k",
    env: { GOOGLE_CLOUD_PROJECT: "renoa-test-project", GOOGLE_CLOUD_LOCATION: "us-central1" },
  };
  assert.equal(
    routedVertexModel(spec, regional).baseUrl,
    "https://us-central1-aiplatform.googleapis.com/v1/projects/renoa-test-project/locations/us-central1",
  );
});

function geminiSse(text: string): string[] {
  return [
    `data: ${JSON.stringify({
      candidates: [{ content: { role: "model", parts: [{ text }] }, finishReason: "STOP" }],
      usageMetadata: { promptTokenCount: 9, candidatesTokenCount: 2, totalTokenCount: 11 },
      modelVersion: HYDROGEN,
    })}`,
  ];
}

async function streamHydrogen(directory: string, origin: string) {
  createStore(directory, KEY, "google-vertex").close();
  const spec = findCatalogModel("google-vertex", HYDROGEN)?.model;
  assert.ok(spec);
  const runtime = loadRuntime({
    provider: "google-vertex",
    modelId: HYDROGEN,
    authStorePath: join(directory, "credentials.sqlite"),
    modelSpec: { ...spec, baseUrl: `${origin}/v1/projects/renoa-test-project/locations/global` },
    reasoningLevel: "medium",
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
  return completed;
}

test("Hydrogen streams through Google's SDK with the key and the medium thinking level", async () => {
  const directory = tempDir();
  const server = await startFakeServer();
  server.enqueue({ sse: geminiSse("hi") });
  try {
    assert.ok(await streamHydrogen(directory.path, server.origin));
    const request = server.requests[0];
    assert.ok(request);
    assert.match(
      request.url ?? "",
      /^\/v1\/projects\/renoa-test-project\/locations\/global\/publishers\/google\/models\/gemini-4-hydrogen:streamGenerateContent/u,
    );
    assert.equal(request.headers["x-goog-api-key"], "vertex-test-key");
    const body = JSON.parse(request.body) as {
      generationConfig?: { thinkingConfig?: { thinkingLevel?: string } };
    };
    assert.equal(body.generationConfig?.thinkingConfig?.thinkingLevel, "MEDIUM");
  } finally {
    await server.close();
    directory.close();
  }
});

test("a dropped Vertex connection may have reached Google, so its outcome is unknown", async () => {
  const directory = tempDir();
  const server = await startFakeServer();
  // Renoa cannot see whether Google's SDK sent the request; a dropped
  // connection would otherwise read as never started. Renoa retries it, so
  // every attempt drops.
  for (let attempt = 0; attempt < 3; attempt += 1) {
    server.enqueue({ reset: true });
  }
  try {
    await assert.rejects(streamHydrogen(directory.path, server.origin), (error: unknown) => {
      assert.ok(error instanceof ProviderFailure);
      assert.equal(error.inferenceOutcome, "unknown");
      return true;
    });
  } finally {
    await server.close();
    directory.close();
  }
});

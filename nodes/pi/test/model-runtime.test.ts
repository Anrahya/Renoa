import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
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
        modelId: "grok-4.5",
        authStorePath: join(directory, "auth.sqlite"),
      }),
      /xai credentials are not configured/,
    );
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

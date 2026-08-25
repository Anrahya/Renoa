import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

import { createStore, loopbackModel, oauthCredential, startFakeServer, successfulChat, tempDir, userRequest } from "./helpers.js";

const bridge = fileURLToPath(new URL("../src/main.js", import.meta.url));

test("compiled bridge catalog, describe, and stream over the process boundary", async () => {
  const server = await startFakeServer();
  server.enqueue({ sse: successfulChat("from-process") });
  const directory = tempDir();
  const authStore = join(directory.path, "credentials.sqlite");
  const store = createStore(directory.path, oauthCredential());
  store.close();
  const model = loopbackModel("xai", "grok-4.6", server.baseUrl);
  try {
    const catalog = await runBridge(
      {
        RENOA_MODEL_ACTION: "catalog",
        RENOA_MODEL_PROVIDER: "xai",
        RENOA_MODEL_AUTH_STORE: authStore,
      },
      "",
    );
    assert.equal(catalog.status, 0, catalog.stderr);
    const catalogRecord = JSON.parse(catalog.stdout) as {
      ok: boolean;
      response: { models: { id: string }[] };
    };
    assert.equal(catalogRecord.ok, true);
    assert.ok(catalogRecord.response.models.some((entry) => entry.id === "grok-4.6"));

    const describe = await runBridge(
      {
        RENOA_MODEL_ACTION: "describe",
        RENOA_MODEL_PROVIDER: "xai",
        RENOA_MODEL: "grok-4.6",
        RENOA_MODEL_AUTH_STORE: authStore,
        RENOA_MODEL_SPEC: JSON.stringify(model),
        RENOA_MODEL_ALLOW_LOOPBACK: "1",
      },
      "",
    );
    assert.equal(describe.status, 0, describe.stderr);
    const description = JSON.parse(describe.stdout) as {
      ok: boolean;
      response: { model_binding_id: string; reasoning_level: string };
    };
    assert.equal(description.ok, true);
    assert.equal(description.response.model_binding_id.length, 64);

    const stream = await runBridge(
      {
        RENOA_MODEL_ACTION: "stream",
        RENOA_MODEL_PROVIDER: "xai",
        RENOA_MODEL: "grok-4.6",
        RENOA_MODEL_AUTH_STORE: authStore,
        RENOA_MODEL_SPEC: JSON.stringify(model),
        RENOA_MODEL_ALLOW_LOOPBACK: "1",
        RENOA_MODEL_MAX_OUTPUT_TOKENS: "128",
      },
      JSON.stringify(userRequest()),
    );
    assert.equal(stream.status, 0, stream.stderr);
    const records = stream.stdout
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line) as { event?: string; response?: { stop_reason?: string } });
    assert.ok(records.some((record) => record.event === "completed"));
  } finally {
    await server.close();
    directory.close();
  }
});

test("malformed stream JSON is invalid_request before credentials are loaded", async () => {
  const directory = tempDir();
  const missingStore = join(directory.path, "missing", "credentials.sqlite");
  try {
    const result = await runBridge(
      {
        RENOA_MODEL_ACTION: "stream",
        RENOA_MODEL_PROVIDER: "xai",
        RENOA_MODEL: "grok-4.6",
        RENOA_MODEL_AUTH_STORE: missingStore,
        RENOA_MODEL_MAX_OUTPUT_TOKENS: "128",
      },
      JSON.stringify({
        system_prompt: 1,
        messages: [{ role: "user", content: "nope" }],
        tools: [],
      }),
    );
    assert.equal(result.status, 1);
    assert.equal(result.stderr.includes("TypeError"), false, result.stderr);
    const record = JSON.parse(result.stdout.trim().split("\n")[0] ?? "{}") as {
      event?: string;
      error_kind?: string;
      error?: string;
    };
    assert.equal(record.event, "error");
    assert.equal(record.error_kind, "invalid_request");
    assert.equal((record.error ?? "").includes("credentials"), false);
  } finally {
    directory.close();
  }
});

function runBridge(
  env: NodeJS.ProcessEnv,
  input: string,
): Promise<{ status: number | null; stdout: string; stderr: string }> {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, ["--dns-result-order=ipv4first", bridge], {
      env: { ...process.env, ...env },
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
    });
    child.on("close", (status) => {
      resolve({ status, stdout, stderr });
    });
    child.stdin.end(input);
  });
}

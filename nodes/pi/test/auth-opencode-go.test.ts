import assert from "node:assert/strict";
import { access, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

import { SqliteCredentialStore } from "../src/credentials.js";

const executable = fileURLToPath(new URL("../src/auth-opencode-go.js", import.meta.url));

test("OpenCode Go enrollment stores and replaces its key without printing it", async () => {
  const directory = await mkdtemp(join(tmpdir(), "renoa-opencode-go-auth-"));
  const storePath = join(directory, "credentials.sqlite");
  try {
    for (const key of ["first-private-key", "replacement-private-key"]) {
      const result = enroll(storePath, `${key}\n`);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, "OpenCode Go API key stored.\n");
      assert.equal(result.stderr, "");
      assert.doesNotMatch(`${result.stdout}${result.stderr}`, new RegExp(key, "u"));
    }

    const store = new SqliteCredentialStore(storePath);
    assert.deepEqual(await store.read("opencode-go"), {
      type: "api_key",
      key: "replacement-private-key",
    });
    store.close();
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

test("OpenCode Go enrollment rejects malformed input without storing it", async () => {
  const directory = await mkdtemp(join(tmpdir(), "renoa-opencode-go-invalid-"));
  const storePath = join(directory, "credentials.sqlite");
  try {
    for (const input of ["", " leading-space\n", "first-line\nsecond-line\n"]) {
      const result = enroll(storePath, input);
      assert.equal(result.status, 1);
      assert.equal(result.stdout, "");
      assert.match(result.stderr, /OpenCode Go API key/u);
      assert.doesNotMatch(result.stderr, /leading-space|first-line|second-line/u);
    }
    await assert.rejects(access(storePath), { code: "ENOENT" });
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

function enroll(storePath: string, input: string) {
  return spawnSync(process.execPath, [executable], {
    encoding: "utf8",
    env: { ...process.env, RENOA_PI_AUTH_STORE: storePath },
    input,
  });
}

import assert from "node:assert/strict";
import { access, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

import { SqliteCredentialStore } from "../src/credentials.js";

const PROVIDERS = [
  { id: "opencode-go", label: "OpenCode Go" },
  { id: "deepseek", label: "DeepSeek" },
] as const;

for (const { id, label } of PROVIDERS) {
  test(`${label} enrollment stores and replaces its key without printing it`, async () => {
    const directory = await mkdtemp(join(tmpdir(), `renoa-${id}-auth-`));
    const storePath = join(directory, "credentials.sqlite");
    try {
      for (const key of ["first-private-key", "replacement-private-key"]) {
        const result = enroll(id, storePath, `${key}\n`);
        assert.equal(result.status, 0, result.stderr);
        assert.equal(result.stdout, `${label} API key stored.\n`);
        assert.equal(result.stderr, "");
        assert.doesNotMatch(`${result.stdout}${result.stderr}`, new RegExp(key, "u"));
      }

      const store = new SqliteCredentialStore(storePath);
      assert.deepEqual(store.read(id), {
        type: "api_key",
        key: "replacement-private-key",
      });
      assert.deepEqual(
        store.list().map((entry) => entry.providerId),
        [id],
      );
      store.close();
    } finally {
      await rm(directory, { force: true, recursive: true });
    }
  });

  test(`${label} enrollment rejects malformed input without storing it`, async () => {
    const directory = await mkdtemp(join(tmpdir(), `renoa-${id}-invalid-`));
    const storePath = join(directory, "credentials.sqlite");
    try {
      for (const input of ["", " leading-space\n", "first-line\nsecond-line\n"]) {
        const result = enroll(id, storePath, input);
        assert.equal(result.status, 1);
        assert.equal(result.stdout, "");
        assert.match(result.stderr, new RegExp(`${label} API key`, "u"));
        assert.doesNotMatch(result.stderr, /leading-space|first-line|second-line/u);
      }
      await assert.rejects(access(storePath), { code: "ENOENT" });
    } finally {
      await rm(directory, { force: true, recursive: true });
    }
  });
}

function enroll(provider: (typeof PROVIDERS)[number]["id"], storePath: string, input: string) {
  const executable = fileURLToPath(new URL(`../src/auth-${provider}.js`, import.meta.url));
  // Node's own runtime warning for node:sqlite is not program output.
  return spawnSync(process.execPath, ["--disable-warning=ExperimentalWarning", executable], {
    encoding: "utf8",
    env: { ...process.env, RENOA_MODEL_AUTH_STORE: storePath },
    input,
  });
}

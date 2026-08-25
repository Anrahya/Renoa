import assert from "node:assert/strict";
import { chmodSync, mkdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";

import { SqliteCredentialStore } from "../src/credentials.js";
import { oauthCredential, tempDir } from "./helpers.js";

test("credential files are created with 0600 permissions", () => {
  const directory = tempDir();
  try {
    const path = join(directory.path, "credentials.sqlite");
    const store = new SqliteCredentialStore(path);
    store.write("xai", oauthCredential());
    store.close();
    if (process.platform !== "win32") {
      assert.equal(statSync(path).mode & 0o777, 0o600);
    }
  } finally {
    directory.close();
  }
});

test("pre-existing credential parent directories keep their mode", () => {
  const directory = tempDir();
  const parent = join(directory.path, "existing");
  mkdirSync(parent);
  chmodSync(parent, 0o755);
  try {
    const store = new SqliteCredentialStore(join(parent, "credentials.sqlite"));
    store.close();
    if (process.platform !== "win32") {
      assert.equal(statSync(parent).mode & 0o777, 0o755);
      assert.equal(statSync(join(parent, "credentials.sqlite")).mode & 0o777, 0o600);
    }
  } finally {
    directory.close();
  }
});

test("concurrent first credential file creation does not fail with EEXIST", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  try {
    const [left, right] = await Promise.all([
      Promise.resolve().then(() => new SqliteCredentialStore(path)),
      Promise.resolve().then(() => new SqliteCredentialStore(path)),
    ]);
    left.write("xai", oauthCredential());
    right.write("xai", oauthCredential(Date.now() + 1));
    left.close();
    right.close();
  } finally {
    directory.close();
  }
});

test("failed OAuth refresh preserves the last valid credential", async () => {
  const directory = tempDir();
  const store = new SqliteCredentialStore(join(directory.path, "credentials.sqlite"));
  try {
    const original = oauthCredential();
    store.write("xai", original);
    await assert.rejects(
      store.refreshOauth("xai", async () => {
        throw new Error("token endpoint unavailable");
      }),
      /token endpoint unavailable/,
    );
    assert.deepEqual(store.read("xai"), original);
    if (process.platform !== "win32") {
      const lockPath = `${join(directory.path, "credentials.sqlite")}.oauth-refresh.sqlite`;
      assert.equal(statSync(lockPath).mode & 0o777, 0o600);
    }
  } finally {
    store.close();
    directory.close();
  }
});

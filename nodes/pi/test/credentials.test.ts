import assert from "node:assert/strict";
import { mkdtemp, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import { SqliteCredentialStore } from "../src/credentials.js";

test("an OAuth credential survives restart in an owner-only store", async () => {
  const directory = await mkdtemp(join(tmpdir(), "renoa-pi-auth-"));
  const path = join(directory, "credentials.sqlite");
  const credential = {
    type: "oauth" as const,
    access: "access-token",
    refresh: "refresh-token",
    expires: 2_000_000_000_000,
    accountId: "account-1",
  };
  try {
    const first = new SqliteCredentialStore(path);
    await first.modify("xai", async (current) => {
      assert.equal(current, undefined);
      return credential;
    });
    first.close();

    assert.equal((await stat(path)).mode & 0o777, 0o600);
    const reopened = new SqliteCredentialStore(path);
    assert.deepEqual(await reopened.read("xai"), credential);
    assert.deepEqual(await reopened.list(), [{ providerId: "xai", type: "oauth" }]);
    reopened.close();
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

test("concurrent OAuth rotations observe the latest stored credential", async () => {
  const directory = await mkdtemp(join(tmpdir(), "renoa-pi-auth-rotation-"));
  const path = join(directory, "credentials.sqlite");
  try {
    const store = new SqliteCredentialStore(path);
    await store.modify("xai", async () => credential(0));
    let releaseFirst: () => void = () => {};
    const firstGate = new Promise<void>((resolve) => {
      releaseFirst = resolve;
    });
    let firstStarted: () => void = () => {};
    const started = new Promise<void>((resolve) => {
      firstStarted = resolve;
    });
    const first = store.modify("xai", async (current) => {
      firstStarted();
      await firstGate;
      return credential(revision(current) + 1);
    });
    await started;
    const second = store.modify("xai", async (current) => credential(revision(current) + 1));

    releaseFirst();
    await Promise.all([first, second]);

    assert.deepEqual(await store.read("xai"), credential(2));
    store.close();
  } finally {
    await rm(directory, { force: true, recursive: true });
  }
});

function credential(revisionValue: number) {
  return {
    type: "oauth" as const,
    access: `access-${revisionValue}`,
    refresh: `refresh-${revisionValue}`,
    expires: 2_000_000_000_000,
    revision: revisionValue,
  };
}

function revision(value: Awaited<ReturnType<SqliteCredentialStore["read"]>>): number {
  assert.ok(value);
  assert.equal(value.type, "oauth");
  if (typeof value.revision !== "number") {
    throw new Error("credential revision is missing");
  }
  return value.revision;
}

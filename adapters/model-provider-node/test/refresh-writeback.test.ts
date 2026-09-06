import assert from "node:assert/strict";
import { join } from "node:path";
import { test } from "node:test";

import { SqliteCredentialStore, type Credential } from "../src/credentials.js";
import { oauthCredential, tempDir } from "./helpers.js";

for (const change of ["delete", "oauth", "account", "api_key", "unchanged"] as const) {
  test(`in-flight refresh respects ${change} through another connection`, async () => {
    const directory = tempDir();
    const path = join(directory.path, "credentials.sqlite");
    const left = new SqliteCredentialStore(path);
    const right = new SqliteCredentialStore(path);
    const started = Promise.withResolvers<void>();
    const release = Promise.withResolvers<void>();
    const original = { ...oauthCredential(100), accountId: "original-account" };
    const refreshed = { ...original, access: "refreshed", refresh: "rotated", expires: 200 };
    let expected: Credential | undefined;
    try {
      left.write("xai", original);
      const pending = left.refreshOauth("xai", async (current) => {
        assert.deepEqual(current, original);
        started.resolve();
        await release.promise;
        return refreshed;
      });
      await started.promise;
      switch (change) {
        case "delete":
          right.delete("xai");
          expected = undefined;
          break;
        case "oauth":
          expected = { ...original, access: "replacement" };
          right.write("xai", expected);
          break;
        case "account":
          expected = { ...original, accountId: "replacement-account" };
          right.write("xai", expected);
          break;
        case "api_key":
          expected = { type: "api_key", key: "fixture-key" };
          right.write("xai", expected);
          break;
        case "unchanged":
          expected = refreshed;
          break;
      }
      if (change === "delete") assert.equal(right.read("xai"), undefined);
      release.resolve();
      assert.deepEqual(await pending, expected);
      assert.deepEqual(left.read("xai"), expected);
      assert.deepEqual(right.read("xai"), expected);
    } finally {
      release.resolve();
      left.close();
      right.close();
      directory.close();
    }
  });
}

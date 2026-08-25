import { existsSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";

import { SqliteCredentialStore } from "../src/credentials.js";

const storePath = process.argv[2];
const tokenUrl = process.argv[3];
const goPath = process.argv[4];
if (storePath === undefined || tokenUrl === undefined) {
  throw new Error("usage: refresh-worker <store-path> <token-url> [go-path]");
}

const store = new SqliteCredentialStore(storePath, { busyTimeoutMs: 5_000 });
try {
  if (goPath !== undefined) {
    while (!existsSync(goPath)) {
      await delay(10);
    }
  }
  const next = await store.refreshOauth("xai", async (current) => {
    const response = await fetch(tokenUrl, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        grant_type: "refresh_token",
        refresh_token: current.refresh,
      }),
    });
    const body = (await response.json()) as {
      access_token?: string;
      refresh_token?: string;
      expires_in?: number;
    };
    if (!response.ok || typeof body.access_token !== "string") {
      throw new Error(`refresh failed: ${response.status}`);
    }
    return {
      type: "oauth",
      access: body.access_token,
      refresh: typeof body.refresh_token === "string" ? body.refresh_token : current.refresh,
      expires: Date.now() + (body.expires_in ?? 3600) * 1000,
    };
  });
  process.stdout.write(`${JSON.stringify(next)}\n`);
} finally {
  store.close();
}

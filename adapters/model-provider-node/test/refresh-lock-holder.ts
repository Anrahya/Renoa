import { writeFileSync } from "node:fs";

import { tryAcquireOauthRefreshLock } from "../src/oauth-refresh-lock.js";

const storePath = process.argv[2];
const readyPath = process.argv[3];
if (storePath === undefined || readyPath === undefined) {
  throw new Error("usage: refresh-lock-holder <store-path> <ready-path>");
}

const lock = tryAcquireOauthRefreshLock(storePath);
if (lock === undefined) {
  throw new Error("refresh lock is already held");
}
writeFileSync(readyPath, "ready\n");
setInterval(() => {}, 60_000);

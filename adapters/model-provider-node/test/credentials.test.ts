import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { join, relative } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

import { SqliteCredentialStore, type OauthCredential, type RefreshClock } from "../src/credentials.js";
import { oauthCredential, tempDir } from "./helpers.js";

test("compare-and-store keeps a newer credential written by another connection", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  const left = new SqliteCredentialStore(path, { busyTimeoutMs: 1_000 });
  const right = new SqliteCredentialStore(path, { busyTimeoutMs: 1_000 });
  try {
    const original = oauthCredential();
    left.write("xai", original);
    const newer = {
      type: "oauth" as const,
      access: "access-from-right",
      refresh: "refresh-from-right",
      expires: Date.now() + 3_600_000,
    };
    const result = await left.refreshOauth("xai", async () => {
      right.write("xai", newer);
      return {
        type: "oauth",
        access: "access-from-left",
        refresh: "refresh-from-left",
        expires: Date.now() + 3_600_000,
      };
    });
    assert.deepEqual(result, newer);
    assert.deepEqual(left.read("xai"), newer);
  } finally {
    left.close();
    right.close();
    directory.close();
  }
});

test("database lock contention surfaces SQLITE_BUSY instead of sleeping", () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  const holder = new SqliteCredentialStore(path, { busyTimeoutMs: 50 });
  const waiter = new SqliteCredentialStore(path, { busyTimeoutMs: 50 });
  try {
    holder.write("xai", oauthCredential());
    const lock = new DatabaseSync(path);
    lock.exec("BEGIN IMMEDIATE");
    try {
      assert.throws(() => waiter.write("xai", oauthCredential(Date.now() + 1)), /busy|locked/i);
    } finally {
      lock.exec("ROLLBACK");
      lock.close();
    }
  } finally {
    holder.close();
    waiter.close();
    directory.close();
  }
});

test("two processes with a single-use rotating refresh token share one refresh", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  const store = new SqliteCredentialStore(path);
  store.write("xai", oauthCredential());
  store.close();

  const accepted: string[] = [];
  const rejected: string[] = [];
  const server = await listenSingleUseTokenServer(accepted, rejected);
  const worker = fileURLToPath(new URL("./refresh-worker.js", import.meta.url));
  const go = join(directory.path, "go");
  try {
    const first = runWorker(worker, path, server.url, go);
    const second = runWorker(worker, path, server.url, go);
    await delay(50);
    writeFileSync(go, "go\n");
    const [left, right] = await Promise.all([first, second]);
    assert.equal(left.status, 0, left.stderr);
    assert.equal(right.status, 0, right.stderr);
    const stored = new SqliteCredentialStore(path);
    const final = stored.read("xai");
    stored.close();
    assert.equal(final?.type, "oauth");
    if (final?.type === "oauth") {
      assert.equal(left.access, final.access);
      assert.equal(right.access, final.access);
      assert.equal(final.access, "access-from-refresh-1");
      assert.equal(final.refresh, "refresh-from-process-1");
    }
    assert.equal(accepted.length, 1);
    assert.equal(rejected.length, 0);
  } finally {
    await server.close();
    directory.close();
  }
});

test("path aliases share one refresh while the holder keeps the lock", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  const clock = new ManualRefreshClock();
  const left = new SqliteCredentialStore(path, { clock });
  const right = new SqliteCredentialStore(relative(process.cwd(), path), { clock });
  const original = oauthCredential();
  left.write("xai", original);
  let refreshes = 0;
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  let started!: () => void;
  const began = new Promise<void>((resolve) => {
    started = resolve;
  });
  try {
    const refresh = async (current: OauthCredential) => {
      refreshes += 1;
      started();
      await gate;
      return {
        type: "oauth" as const,
        access: "access-after-long-refresh",
        refresh: "refresh-after-long-refresh",
        expires: current.expires + 1,
      };
    };
    const leftRefresh = left.refreshOauth("xai", refresh);
    const rightRefresh = right.refreshOauth("xai", refresh);
    await began;
    for (let tick = 0; tick < 8; tick += 1) {
      await clock.advance(50);
    }
    release();
    let settled = false;
    const finished = Promise.all([leftRefresh, rightRefresh]).then((value) => {
      settled = true;
      return value;
    });
    for (let tick = 0; tick < 40 && !settled; tick += 1) {
      await clock.advance(50);
    }
    const [fromLeft, fromRight] = await finished;
    assert.equal(refreshes, 1);
    assert.equal(fromLeft?.type, "oauth");
    assert.equal(fromRight?.type, "oauth");
    if (fromLeft?.type === "oauth" && fromRight?.type === "oauth") {
      assert.equal(fromLeft.access, "access-after-long-refresh");
      assert.equal(fromRight.access, "access-after-long-refresh");
      assert.equal(fromLeft.refresh, "refresh-after-long-refresh");
    }
  } finally {
    left.close();
    right.close();
    directory.close();
  }
});

test("one store serializes concurrent refreshes of the same rotating token", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  const clock = new ManualRefreshClock();
  const store = new SqliteCredentialStore(path, { clock });
  store.write("xai", oauthCredential());
  const labels: string[] = [];
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  let started!: () => void;
  const began = new Promise<void>((resolve) => {
    started = resolve;
  });
  try {
    const first = store.refreshOauth("xai", async (current) => {
      labels.push("a");
      started();
      await gate;
      return {
        type: "oauth",
        access: "access-a",
        refresh: "refresh-a",
        expires: current.expires + 1,
      };
    });
    await began;
    const second = store.refreshOauth("xai", async (current) => {
      labels.push("b");
      return {
        type: "oauth",
        access: "access-b",
        refresh: "refresh-b",
        expires: current.expires + 1,
      };
    });
    for (let tick = 0; tick < 8; tick += 1) {
      await clock.advance(50);
    }
    assert.deepEqual(labels, ["a"]);
    release();
    let settled = false;
    const finished = Promise.all([first, second]).then((value) => {
      settled = true;
      return value;
    });
    for (let tick = 0; tick < 40 && !settled; tick += 1) {
      await clock.advance(50);
    }
    const [fromFirst, fromSecond] = await finished;
    assert.deepEqual(labels, ["a"]);
    assert.equal(fromFirst?.type, "oauth");
    assert.equal(fromSecond?.type, "oauth");
    if (fromFirst?.type === "oauth" && fromSecond?.type === "oauth") {
      assert.equal(fromFirst.access, "access-a");
      assert.equal(fromSecond.access, "access-a");
    }
  } finally {
    store.close();
    directory.close();
  }
});

test("a paused holder does not lose the lock to wall-clock expiry", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  const clock = new ManualRefreshClock();
  const left = new SqliteCredentialStore(path, { clock });
  const right = new SqliteCredentialStore(path, { clock });
  left.write("xai", oauthCredential());
  const labels: string[] = [];
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  let started!: () => void;
  const began = new Promise<void>((resolve) => {
    started = resolve;
  });
  try {
    const leftRefresh = left.refreshOauth("xai", async (current) => {
      labels.push("a");
      started();
      await gate;
      return {
        type: "oauth",
        access: "access-a",
        refresh: "refresh-a",
        expires: current.expires + 1,
      };
    });
    const rightRefresh = right.refreshOauth("xai", async (current) => {
      labels.push("b");
      return {
        type: "oauth",
        access: "access-b",
        refresh: "refresh-b",
        expires: current.expires + 1,
      };
    });
    await began;
    // Advance far past any former 15s lease. Time must not steal from a live
    // holder: rotating refresh tokens are not idempotent.
    for (let tick = 0; tick < 400; tick += 1) {
      await clock.advance(50);
    }
    assert.deepEqual(labels, ["a"]);
    release();
    let settled = false;
    const finished = Promise.all([leftRefresh, rightRefresh]).then((value) => {
      settled = true;
      return value;
    });
    for (let tick = 0; tick < 40 && !settled; tick += 1) {
      await clock.advance(50);
    }
    const [fromLeft, fromRight] = await finished;
    assert.deepEqual(labels, ["a"]);
    assert.equal(fromLeft?.type, "oauth");
    assert.equal(fromRight?.type, "oauth");
    if (fromLeft?.type === "oauth" && fromRight?.type === "oauth") {
      assert.equal(fromLeft.access, "access-a");
      assert.equal(fromRight.access, "access-a");
    }
  } finally {
    left.close();
    right.close();
    directory.close();
  }
});

test("a crashed process releases refresh ownership", async () => {
  const directory = tempDir();
  const path = join(directory.path, "credentials.sqlite");
  const store = new SqliteCredentialStore(path);
  store.write("xai", oauthCredential());
  store.close();
  const readyPath = join(directory.path, "holding");
  const holder = fileURLToPath(new URL("./refresh-lock-holder.js", import.meta.url));
  const child = spawn(process.execPath, [holder, path, readyPath], {
    stdio: ["ignore", "ignore", "pipe"],
  });
  let stderr = "";
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk: string) => {
    stderr += chunk;
  });
  try {
    const deadline = Date.now() + 5_000;
    while (!existsSync(readyPath) && child.exitCode === null && Date.now() < deadline) {
      await delay(10);
    }
    assert.equal(existsSync(readyPath), true, stderr || "lock holder did not become ready");
    const exited = new Promise<void>((resolve) => {
      child.once("close", () => resolve());
    });
    const signal = process.platform === "win32" ? undefined : "SIGKILL";
    assert.equal(child.kill(signal), true);
    let exitTimer: NodeJS.Timeout | undefined;
    try {
      await Promise.race([
        exited,
        new Promise<never>((_resolve, reject) => {
          exitTimer = setTimeout(() => {
            reject(new Error("killed refresh-lock holder did not exit within 5s"));
          }, 5_000);
        }),
      ]);
    } finally {
      clearTimeout(exitTimer);
    }

    const parent = new SqliteCredentialStore(path);
    let refreshes = 0;
    try {
      const next = await parent.refreshOauth("xai", async (current) => {
        refreshes += 1;
        return {
          type: "oauth",
          access: "access-after-crash",
          refresh: "refresh-after-crash",
          expires: current.expires + 1,
        };
      });
      assert.equal(refreshes, 1);
      assert.equal(next?.type, "oauth");
      if (next?.type === "oauth") {
        assert.equal(next.access, "access-after-crash");
      }
    } finally {
      parent.close();
    }
  } finally {
    if (child.exitCode === null && child.signalCode === null) {
      child.kill();
    }
    directory.close();
  }
});

function runWorker(
  worker: string,
  storePath: string,
  tokenUrl: string,
  goPath?: string,
): Promise<{ status: number | null; stderr: string; access?: string }> {
  return new Promise((resolve) => {
    const args = goPath === undefined ? [worker, storePath, tokenUrl] : [worker, storePath, tokenUrl, goPath];
    const child = spawn(process.execPath, args, {
      stdio: ["ignore", "pipe", "pipe"],
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
      let access: string | undefined;
      try {
        const parsed = JSON.parse(stdout) as { access?: string };
        access = parsed.access;
      } catch {
        access = undefined;
      }
      resolve({ status, stderr, ...(access === undefined ? {} : { access }) });
    });
  });
}

function listenSingleUseTokenServer(
  accepted: string[],
  rejected: string[],
): Promise<{ url: string; close(): Promise<void> }> {
  const used = new Set<string>();
  const server = createServer((request, response) => {
    void (async () => {
      const chunks: Buffer[] = [];
      for await (const chunk of request) {
        chunks.push(Buffer.from(chunk));
      }
      const body = new URLSearchParams(Buffer.concat(chunks).toString("utf8"));
      const refreshToken = body.get("refresh_token") ?? "";
      if (used.has(refreshToken) || refreshToken.length === 0) {
        rejected.push(refreshToken);
        response.writeHead(400, { "content-type": "application/json" });
        response.end(JSON.stringify({ error: "invalid_grant" }));
        return;
      }
      used.add(refreshToken);
      const access = `access-from-refresh-${accepted.length + 1}`;
      accepted.push(access);
      response.writeHead(200, { "content-type": "application/json" });
      response.end(
        JSON.stringify({
          access_token: access,
          refresh_token: `refresh-from-process-${accepted.length}`,
          expires_in: 3600,
        }),
      );
    })();
  });
  return new Promise((resolve, reject) => {
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (address === null || typeof address === "string") {
        reject(new Error("token server has no address"));
        return;
      }
      resolve({
        url: `http://127.0.0.1:${address.port}/token`,
        close: () =>
          new Promise((closeResolve, closeReject) => {
            const timer = setTimeout(() => {
              closeReject(new Error("token server close exceeded 5s"));
            }, 5_000);
            server.closeAllConnections();
            server.close((error) => {
              clearTimeout(timer);
              if (error) {
                closeReject(error);
              } else {
                closeResolve();
              }
            });
          }),
      });
    });
  });
}

class ManualRefreshClock implements RefreshClock {
  nowMs = 0;
  readonly waiters: { due: number; resolve: () => void }[] = [];

  sleep(ms: number): Promise<void> {
    return new Promise((resolve) => {
      this.waiters.push({ due: this.nowMs + ms, resolve });
    });
  }

  async advance(ms: number): Promise<void> {
    this.nowMs += ms;
    const due = this.waiters.filter((waiter) => waiter.due <= this.nowMs);
    this.waiters.splice(0, this.waiters.length, ...this.waiters.filter((waiter) => waiter.due > this.nowMs));
    for (const waiter of due) {
      waiter.resolve();
    }
    await Promise.resolve();
    await Promise.resolve();
  }
}

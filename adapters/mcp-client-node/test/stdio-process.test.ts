import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { executeAdapterRequest } from "../src/client.js";
import { MCP_ADAPTER_REVISION, WIRE_VERSION } from "../src/limits.js";
import { runAdapter } from "./support.js";

const STDIO_ENDPOINT = `stdio:${"a".repeat(64)}`;
const TINY_PNG_BASE64 =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

function writeFixtureServer(
  mode:
    | "modern"
    | "legacy"
    | "crash"
    | "noisy-incompatible"
    | "noisy-hang"
    | "notify-then-drop",
): {
  readonly dir: string;
  readonly script: string;
} {
  const dir = mkdtempSync(path.join(os.tmpdir(), "renoa-mcp-stdio-test-"));
  const script = path.join(dir, "server.mjs");
  const source =
    mode === "crash"
      ? `process.stderr.write("fixture stdio boom\\n"); process.exit(7);\n`
      : mode === "noisy-hang"
        ? `process.stderr.write("banner: waiting forever\\n"); setTimeout(() => process.exit(0), 1500);\n`
        : mode === "noisy-incompatible"
        ? `
import { createInterface } from "node:readline";
process.stderr.write("banner: starting incompatible server\\n");
const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });
rl.on("line", (line) => {
  const trimmed = line.trim();
  if (!trimmed) return;
  const msg = JSON.parse(trimmed);
  if (msg.id === undefined) return;
  if (msg.method === "server/discover") {
    process.stdout.write(JSON.stringify({
      jsonrpc: "2.0",
      id: msg.id,
      result: {
        resultType: "complete",
        supportedVersions: ["2026-07-28"],
        capabilities: {},
        ttlMs: 0,
        cacheScope: "private",
      },
    }) + "\\n");
  }
});
`
        : mode === "notify-then-drop"
          ? `
import { createInterface } from "node:readline";
const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });
function send(msg) { process.stdout.write(JSON.stringify(msg) + "\\n"); }
rl.on("line", (line) => {
  const trimmed = line.trim();
  if (!trimmed) return;
  const msg = JSON.parse(trimmed);
  if (msg.id === undefined) return;
  if (msg.method === "server/discover") {
    send({
      jsonrpc: "2.0",
      id: msg.id,
      result: {
        resultType: "complete",
        supportedVersions: ["2026-07-28"],
        capabilities: { tools: {} },
        ttlMs: 0,
        cacheScope: "private",
      },
    });
  } else if (msg.method === "tools/call") {
    send({
      jsonrpc: "2.0",
      method: "notifications/message",
      params: { level: "info", data: "working" },
    });
    setTimeout(() => process.exit(0), 20);
  }
});
`
          : `
import { createInterface } from "node:readline";
const mode = ${JSON.stringify(mode)};
const tinyPng = ${JSON.stringify(TINY_PNG_BASE64)};
const rl = createInterface({ input: process.stdin, crlfDelay: Infinity });
function send(msg) {
  process.stdout.write(JSON.stringify(msg) + "\\n");
}
rl.on("line", (line) => {
  const trimmed = line.trim();
  if (!trimmed) return;
  const msg = JSON.parse(trimmed);
  if (msg.id === undefined) return;
  if (msg.method === "server/discover") {
    if (mode === "legacy") {
      send({ jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: "Method not found" } });
      return;
    }
    send({
      jsonrpc: "2.0",
      id: msg.id,
      result: {
        resultType: "complete",
        supportedVersions: ["2026-07-28", "2025-11-25"],
        capabilities: { tools: {} },
        ttlMs: 0,
        cacheScope: "private",
        _meta: { "io.modelcontextprotocol/serverInfo": { name: "fixture-stdio", version: "1.0.0" } },
      },
    });
    return;
  }
  if (msg.method === "initialize") {
    send({
      jsonrpc: "2.0",
      id: msg.id,
      result: {
        protocolVersion: "2025-11-25",
        capabilities: { tools: {} },
        serverInfo: { name: "fixture-stdio", version: "1.0.0" },
      },
    });
    return;
  }
  if (msg.method === "tools/list") {
    const base = {
      tools: [
        {
          name: "inspect_env",
          description: "Inspect cwd and custom env.",
          inputSchema: {
            type: "object",
            properties: {
              withImage: { type: "boolean" },
            },
          },
        },
      ],
    };
    send({
      jsonrpc: "2.0",
      id: msg.id,
      result: mode === "modern"
        ? { ...base, resultType: "complete", ttlMs: 0, cacheScope: "private" }
        : base,
    });
    return;
  }
  if (msg.method === "tools/call") {
    const args = msg.params?.arguments ?? {};
    const content = [
      {
        type: "text",
        text: JSON.stringify({
          cwd: process.cwd(),
          custom: process.env.CUSTOM_FLAG ?? null,
          leaked: process.env.RENOA_UNRELATED_SECRET ?? null,
        }),
      },
    ];
    if (args.withImage === true) {
      content.push({ type: "image", data: tinyPng, mimeType: "image/png" });
    }
    const result = mode === "modern"
      ? { resultType: "complete", content, isError: false }
      : { content, isError: false };
    send({ jsonrpc: "2.0", id: msg.id, result });
    return;
  }
  send({ jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: "Unknown method" } });
});
`;
  writeFileSync(script, source, "utf8");
  return { dir, script };
}

test("discovers and calls a modern 2026-07-28 stdio MCP server with text, image, cwd, and isolated env", async () => {
  const fixture = writeFixtureServer("modern");
  const prevSecret = process.env.RENOA_UNRELATED_SECRET;
  process.env.RENOA_UNRELATED_SECRET = "must-not-leak";
  try {
    const stdio = {
      command: process.execPath,
      args: [fixture.script],
      env: { CUSTOM_FLAG: "yes" },
      cwd: fixture.dir,
    };
    const discovered = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "discover",
      endpoint: STDIO_ENDPOINT,
      stdio,
    });
    assert.equal(discovered.exitCode, 0);
    assert.equal(discovered.records.length, 1);
    const catalogRecord = discovered.records[0];
    assert.equal(catalogRecord?.event, "discovered");
    assert.equal(catalogRecord.catalog.endpoint, STDIO_ENDPOINT);
    assert.equal(catalogRecord.catalog.protocol_version, "2026-07-28");
    assert.equal(catalogRecord.catalog.adapter_revision, MCP_ADAPTER_REVISION);
    assert.equal(catalogRecord.catalog.tools.length, 1);
    const tool = catalogRecord.catalog.tools[0];
    assert.ok(tool !== undefined);

    const called = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "call",
      endpoint: STDIO_ENDPOINT,
      protocol_version: catalogRecord.catalog.protocol_version,
      stdio,
      tool: { name: tool.name, input_schema: tool.input_schema },
      arguments: { withImage: true },
    });
    assert.equal(called.exitCode, 0);
    assert.equal(called.records.length, 2);
    assert.equal(called.records[0]?.event, "dispatch_started");
    assert.equal(called.records[1]?.event, "completed");
    const result = called.records[1].result;
    assert.equal(result.is_error, false);
    assert.equal(result.content.length, 2);
    assert.equal(result.content[0]?.type, "text");
    const payload = JSON.parse(result.content[0].text) as {
      cwd: string;
      custom: string | null;
      leaked: string | null;
    };
    assert.equal(payload.cwd, fixture.dir);
    assert.equal(payload.custom, "yes");
    assert.equal(payload.leaked, null);
    assert.deepEqual(result.content[1], {
      type: "image",
      mime_type: "image/png",
      data: TINY_PNG_BASE64,
    });
  } finally {
    if (prevSecret === undefined) {
      delete process.env.RENOA_UNRELATED_SECRET;
    } else {
      process.env.RENOA_UNRELATED_SECRET = prevSecret;
    }
    rmSync(fixture.dir, { recursive: true, force: true });
  }
});

test("discovers and calls a legacy 2025-11-25 stdio MCP server", async () => {
  const fixture = writeFixtureServer("legacy");
  try {
    const stdio = { command: process.execPath, args: [fixture.script] };
    const discovered = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "discover",
      endpoint: STDIO_ENDPOINT,
      stdio,
    });
    assert.equal(discovered.exitCode, 0);
    const catalogRecord = discovered.records[0];
    assert.equal(catalogRecord?.event, "discovered");
    assert.equal(catalogRecord.catalog.protocol_version, "2025-11-25");
    const tool = catalogRecord.catalog.tools[0];
    assert.ok(tool !== undefined);

    const called = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "call",
      endpoint: STDIO_ENDPOINT,
      protocol_version: "2025-11-25",
      stdio,
      tool: { name: tool.name, input_schema: tool.input_schema },
      arguments: {},
    });
    assert.equal(called.exitCode, 0);
    assert.equal(called.records[0]?.event, "dispatch_started");
    assert.equal(called.records[1]?.event, "completed");
  } finally {
    rmSync(fixture.dir, { recursive: true, force: true });
  }
});

test("surfaces stderr diagnostics when a stdio MCP server exits during startup", async () => {
  const fixture = writeFixtureServer("crash");
  try {
    const discovered = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "discover",
      endpoint: STDIO_ENDPOINT,
      stdio: { command: process.execPath, args: [fixture.script] },
    });
    assert.equal(discovered.exitCode, 0);
    const failed = discovered.records[0];
    assert.equal(failed?.event, "failed");
    assert.match(failed.failure.message, /fixture stdio boom/);
  } finally {
    rmSync(fixture.dir, { recursive: true, force: true });
  }
});

test("rejects stdio combined with HTTP headers or credentials and malformed stdio endpoints", async () => {
  const withHeaders = await runAdapter({
    wire_version: WIRE_VERSION,
    action: "discover",
    endpoint: STDIO_ENDPOINT,
    headers: { "x-test": "1" },
    stdio: { command: "browse", args: ["mcp"] },
  });
  assert.equal(withHeaders.records[0]?.event, "failed");
  assert.equal(withHeaders.records[0].failure.kind, "invalid_request");

  const badEndpoint = await runAdapter({
    wire_version: WIRE_VERSION,
    action: "discover",
    endpoint: "https://example.com/mcp",
    stdio: { command: "browse", args: ["mcp"] },
  });
  assert.equal(badEndpoint.records[0]?.event, "failed");
  assert.equal(badEndpoint.records[0].failure.kind, "invalid_request");
});

test("preserves incompatible_protocol and cancelled classifications even when stderr is non-empty", async () => {
  const fixture = writeFixtureServer("noisy-incompatible");
  try {
    const discovered = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "discover",
      endpoint: STDIO_ENDPOINT,
      stdio: { command: process.execPath, args: [fixture.script] },
    });
    const failed = discovered.records[0];
    assert.equal(failed?.event, "failed");
    assert.equal(failed.failure.kind, "incompatible_protocol");
    assert.equal(failed.failure.diagnostic.code, "tools_capability_missing");

    const hanging = writeFixtureServer("noisy-hang");
    try {
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), 50);
      const cleanups: Array<() => Promise<void>> = [];
      const record = await executeAdapterRequest(
        {
          wire_version: WIRE_VERSION,
          action: "discover",
          endpoint: STDIO_ENDPOINT,
          stdio: { command: process.execPath, args: [hanging.script] },
        },
        {
          signal: controller.signal,
          dispatchStarted: async () => {},
          registerCleanup: (fn) => cleanups.push(fn),
        },
      );
      clearTimeout(timer);
      for (const cleanup of cleanups) {
        await cleanup().catch(() => {});
      }
      assert.equal(record.event, "failed");
      assert.equal(record.failure.kind, "cancelled");
      assert.equal(record.failure.certainty, "definite");
    } finally {
      rmSync(hanging.dir, { recursive: true, force: true });
    }
  } finally {
    rmSync(fixture.dir, { recursive: true, force: true });
  }
});

test("does not treat a post-dispatch server notification as responseStarted and blocks pre-aborted tools/call dispatch", async () => {
  const fixture = writeFixtureServer("notify-then-drop");
  try {
    const called = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "call",
      endpoint: STDIO_ENDPOINT,
      protocol_version: "2026-07-28",
      stdio: { command: process.execPath, args: [fixture.script] },
      tool: { name: "noop", input_schema: { type: "object" } },
      arguments: {},
    });
    assert.equal(called.records[0]?.event, "dispatch_started");
    assert.equal(called.records[1]?.event, "failed");
    assert.equal(called.records[1].failure.kind, "transport");
    assert.equal(called.records[1].failure.certainty, "unknown");
    assert.equal(called.records[1].failure.partial_changes_possible, true);

    const modern = writeFixtureServer("modern");
    try {
      const controller = new AbortController();
      let dispatched = false;
      const cleanups: Array<() => Promise<void>> = [];
      const record = await executeAdapterRequest(
        {
          wire_version: WIRE_VERSION,
          action: "call",
          endpoint: STDIO_ENDPOINT,
          protocol_version: "2026-07-28",
          stdio: { command: process.execPath, args: [modern.script] },
          tool: { name: "inspect_env", input_schema: { type: "object" } },
          arguments: {},
        },
        {
          signal: controller.signal,
          dispatchStarted: async () => {
            dispatched = true;
          },
          registerCleanup: (fn) => {
            cleanups.push(fn);
            // Abort immediately once connected/registered before call dispatches.
            controller.abort();
          },
        },
      );
      for (const cleanup of cleanups) {
        await cleanup().catch(() => {});
      }
      assert.equal(dispatched, false);
      assert.equal(record.event, "failed");
      assert.equal(record.failure.kind, "cancelled");
      assert.equal(record.failure.certainty, "definite");
      assert.equal(record.failure.partial_changes_possible, false);
    } finally {
      rmSync(modern.dir, { recursive: true, force: true });
    }
  } finally {
    rmSync(fixture.dir, { recursive: true, force: true });
  }
});

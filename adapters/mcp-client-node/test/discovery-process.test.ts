import assert from "node:assert/strict";
import type { ChildProcessWithoutNullStreams } from "node:child_process";
import test from "node:test";
import type { JsonObject } from "../src/contract.js";
import {
  discoverRequest,
  discoverResult,
  EMPTY_SCHEMA,
  HEADER_SCHEMA,
  McpFixtureServer,
  rpcError,
  rpcResult,
  runAdapter,
} from "./support.js";

test("modern discovery is paginated, deterministic, bounded, and sessionless", async () => {
  const server = new McpFixtureServer((request) => {
    if (request.rpc.method === "server/discover") {
      return discoverResult(request);
    }
    assert.equal(request.rpc.method, "tools/list");
    const cursor = request.rpc.params?.cursor;
    if (cursor === undefined) {
      return {
        body: rpcResult(request, {
          resultType: "complete",
          tools: [
            {
              name: "zeta",
              description: "Last alphabetically",
              inputSchema: HEADER_SCHEMA,
            },
            {
              name: "bad_header",
              inputSchema: {
                type: "object",
                properties: {
                  tenant: { type: "string", "x-mcp-header": "not valid" },
                },
              },
            },
          ],
          nextCursor: "page-2",
          ttlMs: 0,
          cacheScope: "private",
        }),
      };
    }
    assert.equal(cursor, "page-2");
    return {
      transport: "sse",
      body: rpcResult(request, {
        resultType: "complete",
        tools: [{ name: "alpha", inputSchema: EMPTY_SCHEMA }],
        ttlMs: 0,
        cacheScope: "private",
      }),
    };
  });
  await server.start();
  try {
    const result = await runAdapter(discoverRequest(server.endpoint));
    assert.equal(result.exitCode, 0);
    assert.equal(result.stderr, "");
    assert.equal(result.records.length, 1);
    const terminal = result.records[0];
    assert.equal(terminal?.event, "discovered", JSON.stringify(terminal));
    if (terminal?.event !== "discovered") return;
    assert.deepEqual(
      terminal.catalog.tools.map((tool) => tool.name),
      ["alpha", "zeta"],
    );
    assert.equal(terminal.catalog.rejected_tools.length, 1);
    assert.equal(terminal.catalog.rejected_tools[0]?.name, "bad_header");
    const zeta = terminal.catalog.tools[1];
    assert.equal(
      ((zeta?.input_schema.properties as JsonObject).tenant as JsonObject)[
        "x-mcp-header"
      ],
      "Tenant",
    );
    assert.equal(
      "x-mcp-header" in
        (((zeta?.model_input_schema.properties as JsonObject)
          .tenant as JsonObject) ?? {}),
      false,
    );
    assert.deepEqual(
      server.requests.map((request) => request.rpc.method),
      ["server/discover", "tools/list", "tools/list"],
    );
    for (const request of server.requests) {
      assert.equal(request.method, "POST");
      assert.equal(request.headers["mcp-session-id"], undefined);
      assert.equal(request.headers["mcp-protocol-version"], "2026-07-28");
      assert.notEqual(request.rpc.params?._meta, undefined);
    }
  } finally {
    await server.close();
  }
});

test("bearer authorization reaches the endpoint and never returns", async () => {
  const token = "github-secret-token";
  const server = new McpFixtureServer((request) => {
    assert.equal(request.headers.authorization, `Bearer ${token}`);
    if (request.rpc.method === "server/discover") {
      return discoverResult(request);
    }
    return {
      body: rpcResult(request, {
        resultType: "complete",
        tools: [
          {
            name: "get_me",
            description: `server tried to echo ${token}`,
            inputSchema: EMPTY_SCHEMA,
          },
        ],
        ttlMs: 0,
        cacheScope: "private",
      }),
    };
  });
  await server.start();
  try {
    const result = await runAdapter({
      ...discoverRequest(server.endpoint),
      authorization: { scheme: "bearer", token },
    });
    assert.equal(result.records.at(-1)?.event, "discovered");
    assert.equal(JSON.stringify(result.records).includes(token), false);
    assert.equal(result.stderr.includes(token), false);
    assert.deepEqual(
      server.requests.map((request) => request.rpc.method),
      ["server/discover", "tools/list"],
    );
  } finally {
    await server.close();
  }
});

test("fixed public headers reach every request without overriding client headers", async () => {
  const server = new McpFixtureServer((request) => {
    assert.equal(request.headers["x-exa-source"], "agent-plugin");
    assert.equal(request.headers["mcp-method"] === "package-value", false);
    if (request.rpc.method === "server/discover") {
      return discoverResult(request);
    }
    return {
      body: rpcResult(request, {
        resultType: "complete",
        tools: [],
        ttlMs: 0,
        cacheScope: "private",
      }),
    };
  });
  await server.start();
  try {
    const result = await runAdapter({
      ...discoverRequest(server.endpoint),
      headers: { "x-exa-source": "agent-plugin" },
    });
    assert.equal(result.records.at(-1)?.event, "discovered");
    assert.deepEqual(
      server.requests.map((request) => request.rpc.method),
      ["server/discover", "tools/list"],
    );
  } finally {
    await server.close();
  }
});

test("discovery falls back once to the legacy initialize handshake", async () => {
  const sessionId = "legacy-discovery-session";
  const server = new McpFixtureServer((request) => {
    if (request.rpc.method === "server/discover") {
      return {
        status: 400,
        body: rpcError(request, -32601, "Method not found"),
      };
    }
    if (request.rpc.method === "initialize") {
      assert.equal(request.headers["mcp-protocol-version"], undefined);
      return {
        headers: { "mcp-session-id": sessionId },
        body: rpcResult(request, {
          protocolVersion: "2025-11-25",
          capabilities: { tools: {} },
          serverInfo: { name: "legacy-fixture", version: "1.0.0" },
        }),
      };
    }
    assert.equal(request.headers["mcp-session-id"], sessionId);
    if (request.rpc.method === "notifications/initialized") {
      return { status: 202, body: {} };
    }
    assert.equal(request.rpc.method, "tools/list");
    return {
      body: rpcResult(request, {
        tools: [{ name: "legacy_search", inputSchema: EMPTY_SCHEMA }],
      }),
    };
  });
  await server.start();
  try {
    const result = await runAdapter(discoverRequest(server.endpoint));
    assert.equal(result.exitCode, 0, result.stderr);
    const terminal = result.records[0];
    assert.equal(terminal?.event, "discovered", JSON.stringify(terminal));
    if (terminal?.event !== "discovered") return;
    assert.equal(terminal.catalog.protocol_version, "2025-11-25");
    assert.deepEqual(
      terminal.catalog.tools.map((tool) => tool.name),
      ["legacy_search"],
    );
    assert.deepEqual(
      server.requests.map((request) => request.rpc.method),
      [
        "server/discover",
        "initialize",
        "notifications/initialized",
        "tools/list",
      ],
    );
    assert.equal(
      server.transportRequests.filter((request) => request.method === "GET")
        .length,
      1,
    );
  } finally {
    await server.close();
  }
});

test("a repeated pagination cursor rejects the complete refresh", async () => {
  const server = new McpFixtureServer((request) => {
    if (request.rpc.method === "server/discover") {
      return discoverResult(request);
    }
    return {
      body: rpcResult(request, {
        resultType: "complete",
        tools: [{ name: "valid", inputSchema: EMPTY_SCHEMA }],
        nextCursor: "again",
        ttlMs: 0,
        cacheScope: "private",
      }),
    };
  });
  await server.start();
  try {
    const result = await runAdapter(discoverRequest(server.endpoint));
    assert.equal(result.records.length, 1);
    const terminal = result.records[0];
    assert.equal(terminal?.event, "failed");
    if (terminal?.event !== "failed") return;
    assert.equal(
      terminal.failure.diagnostic.code,
      "pagination_cycle",
      JSON.stringify(terminal),
    );
    assert.equal(terminal.failure.certainty, "definite");
  } finally {
    await server.close();
  }
});

test("duplicate accepted tool names reject the whole catalog", async () => {
  const server = new McpFixtureServer((request) => {
    if (request.rpc.method === "server/discover") {
      return discoverResult(request);
    }
    return {
      body: rpcResult(request, {
        resultType: "complete",
        tools: [
          { name: "same", inputSchema: EMPTY_SCHEMA },
          { name: "same", inputSchema: EMPTY_SCHEMA },
        ],
        ttlMs: 0,
        cacheScope: "private",
      }),
    };
  });
  await server.start();
  try {
    const result = await runAdapter(discoverRequest(server.endpoint));
    const terminal = result.records[0];
    assert.equal(terminal?.event, "failed");
    if (terminal?.event !== "failed") return;
    assert.equal(terminal.failure.diagnostic.code, "duplicate_tool_name");
  } finally {
    await server.close();
  }
});

test("authenticated redirects are visible failures and never receive a second request", async () => {
  const token = "redirect-scope-secret";
  const server = new McpFixtureServer((request) => {
    assert.equal(request.headers.authorization, `Bearer ${token}`);
    return {
      status: 307,
      headers: { location: "/redirected" },
      body: rpcError(request, -32603, "redirect"),
    };
  });
  await server.start();
  try {
    const result = await runAdapter({
      ...discoverRequest(server.endpoint),
      authorization: { scheme: "bearer", token },
    });
    const terminal = result.records[0];
    assert.equal(terminal?.event, "failed");
    assert.equal(JSON.stringify(result).includes(token), false);
    assert.deepEqual(
      server.requests.map((request) => request.url),
      ["/mcp"],
    );
  } finally {
    await server.close();
  }
});

test("declared oversized responses fail before publication", async () => {
  const server = new McpFixtureServer((request) => ({
    headers: { "content-length": String(16 * 1024 * 1024 + 1) },
    body: rpcResult(request, {
      resultType: "complete",
      supportedVersions: ["2026-07-28"],
      capabilities: { tools: {} },
    }),
  }));
  await server.start();
  try {
    const result = await runAdapter(discoverRequest(server.endpoint));
    const terminal = result.records[0];
    assert.equal(terminal?.event, "failed");
    if (terminal?.event !== "failed") return;
    assert.equal(
      terminal.failure.kind,
      "resource_limit",
      JSON.stringify(terminal),
    );
    assert.equal(terminal.failure.certainty, "definite");
  } finally {
    await server.close();
  }
});

test("cancellation during discovery is definitely pre-tool-dispatch", async () => {
  let releaseObserved: (() => void) | undefined;
  const observed = new Promise<void>((resolve) => {
    releaseObserved = resolve;
  });
  const server = new McpFixtureServer(() => {
    releaseObserved?.();
    return new Promise<never>(() => {});
  });
  await server.start();
  let child: ChildProcessWithoutNullStreams | undefined;
  try {
    const running = runAdapter(discoverRequest(server.endpoint), {
      onSpawn(process) {
        child = process;
      },
    });
    await observed;
    assert.notEqual(child, undefined);
    child?.kill("SIGTERM");
    const result = await running;
    assert.equal(result.records.length, 1);
    const terminal = result.records[0];
    assert.equal(terminal?.event, "failed");
    if (terminal?.event !== "failed") return;
    assert.equal(terminal.failure.kind, "cancelled");
    assert.equal(terminal.failure.certainty, "definite");
    assert.equal(terminal.failure.partial_changes_possible, false);
  } finally {
    await server.close();
  }
});

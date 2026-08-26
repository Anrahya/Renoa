import assert from "node:assert/strict";
import type { ChildProcessWithoutNullStreams } from "node:child_process";
import test from "node:test";
import {
  callRequest,
  discoverResult,
  McpFixtureServer,
  rpcError,
  rpcResult,
  runAdapter,
} from "./support.js";

test("one frozen call emits dispatch first and preserves ordered mixed content", async () => {
  const server = new McpFixtureServer((request) => {
    if (request.rpc.method === "server/discover") {
      return discoverResult(request);
    }
    assert.equal(request.rpc.method, "tools/call");
    return {
      transport: "sse",
      body: rpcResult(request, {
        resultType: "complete",
        content: [
          { type: "text", text: "before" },
          { type: "image", data: "aW1hZ2U=", mimeType: "image/png" },
          { type: "text", text: "before" },
        ],
        structuredContent: null,
      }),
    };
  });
  await server.start();
  try {
    const result = await runAdapter(callRequest(server.endpoint));
    assert.equal(result.exitCode, 0);
    assert.equal(result.stderr, "");
    assert.deepEqual(
      result.records.map((record) => record.event),
      ["dispatch_started", "completed"],
    );
    const terminal = result.records[1];
    assert.equal(terminal?.event, "completed");
    if (terminal?.event !== "completed") return;
    assert.deepEqual(terminal.result.content, [
      { type: "text", text: "before" },
      { type: "image", data: "aW1hZ2U=", mime_type: "image/png" },
      { type: "text", text: "before" },
    ]);
    assert.deepEqual(terminal.result.structured_content, {
      present: true,
      value: null,
    });
    assert.deepEqual(
      server.requests.map((request) => request.rpc.method),
      ["server/discover", "tools/call"],
    );
    const call = server.requests[1];
    assert.equal(call?.headers["mcp-param-tenant"], "renoa");
    assert.equal(call?.headers["mcp-method"], "tools/call");
    assert.equal(call?.headers["mcp-name"], "write_note");
  } finally {
    await server.close();
  }
});

test("a redirect after tool dispatch is unknown and is not followed", async () => {
  const server = new McpFixtureServer((request) => {
    if (request.rpc.method === "server/discover") {
      return discoverResult(request);
    }
    return {
      status: 307,
      headers: { location: "/redirected" },
      body: rpcError(request, -32603, "redirect"),
    };
  });
  await server.start();
  try {
    const result = await runAdapter(callRequest(server.endpoint));
    const terminal = result.records.at(-1);
    assert.equal(terminal?.event, "failed");
    if (terminal?.event !== "failed") return;
    assert.equal(terminal.failure.certainty, "unknown");
    assert.deepEqual(
      server.requests.map((request) => request.url),
      ["/mcp", "/mcp"],
    );
  } finally {
    await server.close();
  }
});

test("header mismatch cannot trigger an SDK tools/call retry", async () => {
  const server = new McpFixtureServer((request) => {
    if (request.rpc.method === "server/discover") {
      return discoverResult(request);
    }
    assert.equal(request.rpc.method, "tools/call");
    return { status: 400, body: rpcError(request, -32020, "Header mismatch") };
  });
  await server.start();
  try {
    const result = await runAdapter(callRequest(server.endpoint));
    assert.deepEqual(
      result.records.map((record) => record.event),
      ["dispatch_started", "failed"],
    );
    const terminal = result.records[1];
    assert.equal(terminal?.event, "failed");
    if (terminal?.event !== "failed") return;
    assert.equal(terminal.failure.certainty, "definite");
    assert.equal(terminal.failure.partial_changes_possible, true);
    assert.deepEqual(
      server.requests.map((request) => request.rpc.method),
      ["server/discover", "tools/call"],
    );
  } finally {
    await server.close();
  }
});

test("connection loss after dispatch is reported as outcome unknown", async () => {
  const server = new McpFixtureServer((request) => {
    if (request.rpc.method === "server/discover") {
      return discoverResult(request);
    }
    return { body: {}, destroySocket: true };
  });
  await server.start();
  try {
    const result = await runAdapter(callRequest(server.endpoint));
    assert.deepEqual(
      result.records.map((record) => record.event),
      ["dispatch_started", "failed"],
    );
    const terminal = result.records[1];
    assert.equal(terminal?.event, "failed");
    if (terminal?.event !== "failed") return;
    assert.equal(terminal.failure.certainty, "unknown");
    assert.equal(terminal.failure.partial_changes_possible, true);
  } finally {
    await server.close();
  }
});

test("cancellation after observed dispatch is outcome unknown", async () => {
  let releaseObserved: (() => void) | undefined;
  const observed = new Promise<void>((resolve) => {
    releaseObserved = resolve;
  });
  const server = new McpFixtureServer((request) => {
    if (request.rpc.method === "server/discover") {
      return discoverResult(request);
    }
    releaseObserved?.();
    return new Promise<never>(() => {});
  });
  await server.start();
  let child: ChildProcessWithoutNullStreams | undefined;
  try {
    const running = runAdapter(callRequest(server.endpoint), {
      onSpawn(process) {
        child = process;
      },
    });
    await observed;
    assert.notEqual(child, undefined);
    child?.kill("SIGTERM");
    const result = await running;
    assert.deepEqual(
      result.records.map((record) => record.event),
      ["dispatch_started", "failed"],
    );
    const terminal = result.records[1];
    assert.equal(terminal?.event, "failed");
    if (terminal?.event !== "failed") return;
    assert.equal(terminal.failure.kind, "cancelled");
    assert.equal(terminal.failure.certainty, "unknown");
  } finally {
    await server.close();
  }
});

test("input-required is visible and is never auto-fulfilled or retried", async () => {
  const server = new McpFixtureServer((request) => {
    if (request.rpc.method === "server/discover") {
      return discoverResult(request);
    }
    return {
      body: rpcResult(request, {
        resultType: "input_required",
        requestState: "opaque",
      }),
    };
  });
  await server.start();
  try {
    const result = await runAdapter(callRequest(server.endpoint));
    const terminal = result.records.at(-1);
    assert.equal(terminal?.event, "failed");
    if (terminal?.event !== "failed") return;
    assert.equal(terminal.failure.kind, "unsupported_result");
    assert.equal(terminal.failure.certainty, "definite");
    assert.equal(terminal.failure.partial_changes_possible, true);
    assert.equal(
      server.requests.filter((request) => request.rpc.method === "tools/call")
        .length,
      1,
    );
  } finally {
    await server.close();
  }
});

test("output-schema validation fails after one completed remote execution", async () => {
  const server = new McpFixtureServer((request) => {
    if (request.rpc.method === "server/discover") {
      return discoverResult(request);
    }
    return {
      body: rpcResult(request, {
        resultType: "complete",
        content: [{ type: "text", text: "remote work finished" }],
        structuredContent: { count: "not-an-integer" },
      }),
    };
  });
  await server.start();
  try {
    const request = callRequest(server.endpoint);
    assert.equal(request.action, "call");
    if (request.action !== "call") return;
    const result = await runAdapter({
      ...request,
      tool: {
        ...request.tool,
        output_schema: {
          type: "object",
          properties: { count: { type: "integer" } },
          required: ["count"],
        },
      },
    });
    const terminal = result.records.at(-1);
    assert.equal(terminal?.event, "failed");
    if (terminal?.event !== "failed") return;
    assert.equal(terminal.failure.certainty, "definite");
    assert.equal(terminal.failure.partial_changes_possible, true);
    assert.equal(
      server.requests.filter((observed) => observed.rpc.method === "tools/call")
        .length,
      1,
    );
  } finally {
    await server.close();
  }
});

test("a valid terminal result does not wait for SSE EOF", async () => {
  const server = new McpFixtureServer((request) => {
    if (request.rpc.method === "server/discover") {
      return discoverResult(request);
    }
    return {
      transport: "sse",
      keepOpenAfterBody: true,
      body: rpcResult(request, {
        resultType: "complete",
        content: [{ type: "text", text: "finished" }],
      }),
    };
  });
  await server.start();
  try {
    const result = await runAdapter(callRequest(server.endpoint));
    assert.deepEqual(
      result.records.map((record) => record.event),
      ["dispatch_started", "completed"],
    );
    assert.equal(result.exitCode, 0);
  } finally {
    await server.close();
  }
});

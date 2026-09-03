import assert from "node:assert/strict";
import test from "node:test";
import {
  callRequest,
  discoverResult,
  McpFixtureServer,
  rpcResult,
  runAdapter,
} from "./support.js";

test("one insufficient-scope response is actionable and never retries the tool", async () => {
  const server = new McpFixtureServer((request) => {
    if (request.rpc.method === "server/discover") {
      return discoverResult(request);
    }
    assert.equal(request.rpc.method, "tools/call");
    return {
      status: 403,
      headers: {
        "www-authenticate":
          'Bearer error="insufficient_scope", scope="bookmark.write users.read", error_description="Write permission is required"',
      },
      body: rpcResult(request, {
        content: [{ type: "text", text: "insufficient scope" }],
        isError: true,
      }),
    };
  });
  await server.start();
  try {
    const result = await runAdapter(callRequest(server.endpoint));
    assert.deepEqual(
      result.records.map((record) => record.event),
      ["dispatch_started", "failed"],
    );
    const record = result.records[1];
    assert.equal(record?.event, "failed", JSON.stringify(record));
    if (record?.event !== "failed") return;
    assert.equal(record.failure.certainty, "definite");
    assert.equal(record.failure.partial_changes_possible, false);
    assert.equal(record.failure.diagnostic.code, "oauth_insufficient_scope");
    assert.equal(record.failure.diagnostic.http_status, 403);
    assert.equal(
      record.failure.diagnostic.required_scope,
      "bookmark.write users.read",
    );
    assert.equal(
      server.requests.filter((request) => request.rpc.method === "tools/call")
        .length,
      1,
    );
  } finally {
    await server.close();
  }
});

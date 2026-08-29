import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { fileURLToPath } from "node:url";
import test from "node:test";

import { execute } from "../src/client.js";
import { RegistryProblem } from "../src/errors.js";
import { getJson, registryBaseUrl } from "../src/http.js";
import { parseRequest } from "../src/wire.js";

type Handler = (
  request: IncomingMessage,
  response: ServerResponse,
) => void | Promise<void>;

test("search normalizes a human query without exposing endpoint metadata", async () => {
  const requests: URL[] = [];
  await withServer((request, response) => {
    const url = requestUrl(request);
    requests.push(url);
    const search = url.searchParams.get("search");
    const servers =
      search === "google-drive"
        ? [registryRecord("io.github.example/google-drive", "1.2.3")]
        : search === "google"
          ? [registryRecord("com.googleapis.compute/mcp", "1.0.0")]
          : [];
    respondJson(response, { servers, metadata: { count: servers.length } });
  }, async (baseUrl) => {
    const record = await execute(
      { wire_version: 1, action: "search", query: "I need to connect Google Drive MCP" },
      { baseUrl },
    );
    assert.equal(record.event, "completed");
    if (record.event !== "completed" || record.result.action !== "search") {
      return;
    }
    assert.deepEqual(record.result.normalized_queries, [
      "google-drive",
      "googledrive",
      "google",
      "drive",
    ]);
    assert.equal(record.result.candidates.length, 1);
    assert.equal(record.result.candidates[0]?.registry_name, "io.github.example/google-drive");
    assert.equal("remotes" in (record.result.candidates[0] ?? {}), false);
    assert.match(record.result.next_action, /verify provider ownership/u);
  });
  assert.deepEqual(
    requests.map((url) => url.searchParams.get("search")).sort(),
    ["drive", "google", "google-drive", "googledrive"],
  );
  for (const request of requests) {
    assert.equal(request.pathname, "/v0.1/servers");
    assert.equal(request.searchParams.get("version"), "latest");
    assert.equal(request.searchParams.get("limit"), "100");
  }
});

test("search follows cursors, deduplicates exact versions, and ranks publisher matches", async () => {
  const community = registryRecord("io.github.someone/cloudflare", "2.0.0");
  const official = registryRecord("com.cloudflare.mcp/mcp", "1.0.0", {
    remotes: [{ type: "streamable-http", url: "https://docs.mcp.cloudflare.com/mcp" }],
  });
  await withServer((request, response) => {
    const cursor = requestUrl(request).searchParams.get("cursor");
    respondJson(
      response,
      cursor === null
        ? { servers: [community], metadata: { count: 1, nextCursor: "page-two" } }
        : { servers: [community, official], metadata: { count: 2 } },
    );
  }, async (baseUrl) => {
    const record = await execute(
      { wire_version: 1, action: "search", query: "cloudflare" },
      { baseUrl },
    );
    assert.equal(record.event, "completed");
    if (record.event !== "completed" || record.result.action !== "search") {
      return;
    }
    assert.deepEqual(
      record.result.candidates.map((candidate) => candidate.registry_name),
      ["com.cloudflare.mcp/mcp", "io.github.someone/cloudflare"],
    );
    assert.equal(
      record.result.candidates[0]?.publisher_namespace_matches_query,
      true,
    );
    assert.deepEqual(record.result.coverage, {
      returned: 2,
      unique_seen: 2,
      rejected_records: 0,
      filtered_records: 0,
      source_truncated: false,
      output_truncated: false,
    });
  });
});

test("search rejects conflicting metadata for one exact version", async () => {
  await withServer((_request, response) => {
    respondJson(response, {
      servers: [
        registryRecord("com.example/search", "1.0.0", { description: "first" }),
        registryRecord("com.example/search", "1.0.0", { description: "second" }),
      ],
      metadata: { count: 2 },
    });
  }, async (baseUrl) => {
    await assert.rejects(
      execute(
        { wire_version: 1, action: "search", query: "search" },
        { baseUrl },
      ),
      (error: unknown) =>
        error instanceof RegistryProblem && error.code === "conflicting_registry_record",
    );
  });
});

test("search trims only at the explicit model-facing byte boundary", async () => {
  await withServer((_request, response) => {
    respondJson(response, {
      servers: Array.from({ length: 100 }, (_, index) =>
        registryRecord(`com.bulk/search-${String(index).padStart(3, "0")}`, "1.0.0", {
          description: `search ${"x".repeat(1_000)}`,
        }),
      ),
      metadata: { count: 100 },
    });
  }, async (baseUrl) => {
    const record = await execute(
      { wire_version: 1, action: "search", query: "search" },
      { baseUrl },
    );
    assert.equal(record.event, "completed");
    if (record.event !== "completed" || record.result.action !== "search") {
      return;
    }
    assert.ok(record.result.candidates.length > 5);
    assert.ok(record.result.candidates.length < 100);
    assert.equal(record.result.coverage.unique_seen, 100);
    assert.equal(record.result.coverage.output_truncated, true);
    assert.ok(Buffer.byteLength(JSON.stringify(record.result), "utf8") <= 44 * 1_024);
  });
});

test("exact lookup preserves safe endpoint requirements and never copies secret values", async () => {
  const seenPaths: string[] = [];
  await withServer((request, response) => {
    seenPaths.push(request.url ?? "");
    respondJson(
      response,
      registryRecord("com.cloudflare.mcp/mcp", "1.0.0", {
        status: "deleted",
        remotes: [
          {
            type: "streamable-http",
            url: "https://docs.mcp.cloudflare.com/mcp",
            headers: [
              {
                name: "Authorization",
                description: "Optional bearer token",
                isRequired: false,
                isSecret: true,
                value: "Bearer MUST-NOT-LEAK",
              },
            ],
          },
          { type: "streamable-http", url: "https://{account}.example.com/mcp" },
          { type: "sse", url: "https://docs.mcp.cloudflare.com/sse" },
          { type: "websocket", url: "https://example.com/mcp" },
        ],
        packages: [
          {
            registryType: "npm",
            identifier: "@example/server",
            version: "1.0.0",
            transport: { type: "stdio" },
          },
        ],
      }),
    );
  }, async (baseUrl) => {
    const result = await execute(
      {
        wire_version: 1,
        action: "lookup",
        registry_name: "com.cloudflare.mcp/mcp",
        registry_version: "1.0.0",
      },
      { baseUrl },
    );
    assert.equal(result.event, "completed");
    if (result.event !== "completed" || result.result.action !== "lookup") {
      return;
    }
    assert.equal(result.result.record.remotes[0]?.transport_supported, true);
    assert.equal(result.result.record.status, "deleted");
    assert.equal(result.result.record.remotes[0]?.headers[0]?.secret, true);
    assert.equal(result.result.record.remotes[1]?.blocker, "endpoint_template_unsupported");
    assert.equal(result.result.record.remotes[2]?.blocker, "sse_transport_unsupported");
    assert.equal(result.result.record.remotes[3]?.transport, "unknown");
    assert.equal(result.result.record.remotes[3]?.declared_transport, "websocket");
    assert.equal(result.result.record.remotes[3]?.blocker, "unsupported_transport");
    assert.equal(result.result.record.packages[0]?.supported_by_renoa, false);
    assert.doesNotMatch(JSON.stringify(result), /MUST-NOT-LEAK/u);
    assert.deepEqual(result.result.trust, {
      verified: "publisher_namespace_control",
      not_verified: [
        "provider_endorsement",
        "metadata_accuracy",
        "server_safety",
        "endpoint_behavior",
      ],
    });
  });
  assert.deepEqual(seenPaths, [
    "/v0.1/servers/com.cloudflare.mcp%2Fmcp/versions/1.0.0",
  ]);
});

test("templated endpoints cannot hide credentials", async () => {
  await withServer((_request, response) => {
    respondJson(
      response,
      registryRecord("com.example/unsafe", "1.0.0", {
        remotes: [
          {
            type: "streamable-http",
            url: "https://user:password@{tenant}.example.com/mcp",
          },
        ],
      }),
    );
  }, async (baseUrl) => {
    await assert.rejects(
      execute(
        {
          wire_version: 1,
          action: "lookup",
          registry_name: "com.example/unsafe",
          registry_version: "1.0.0",
        },
        { baseUrl },
      ),
      (error: unknown) =>
        error instanceof RegistryProblem && /credentials/u.test(error.message),
    );
  });
});

test("lookup never returns concrete endpoint query values", async () => {
  await withServer((_request, response) => {
    respondJson(
      response,
      registryRecord("com.example/unsafe-query", "1.0.0", {
        remotes: [
          {
            type: "streamable-http",
            url: "https://example.com/mcp?access_token=MUST-NOT-LEAK",
          },
        ],
      }),
    );
  }, async (baseUrl) => {
    await assert.rejects(
      execute(
        {
          wire_version: 1,
          action: "lookup",
          registry_name: "com.example/unsafe-query",
          registry_version: "1.0.0",
        },
        { baseUrl },
      ),
      (error: unknown) => {
        assert.ok(error instanceof RegistryProblem);
        assert.match(error.message, /concrete query parameters/u);
        assert.doesNotMatch(error.message, /MUST-NOT-LEAK/u);
        return true;
      },
    );
  });
});

test("lookup never returns concrete package URL query values", async () => {
  await withServer((_request, response) => {
    respondJson(
      response,
      registryRecord("com.example/unsafe-package", "1.0.0", {
        packages: [
          {
            registryType: "mcpb",
            identifier: "https://example.com/server.mcpb?token=MUST-NOT-LEAK",
            transport: { type: "stdio" },
          },
        ],
      }),
    );
  }, async (baseUrl) => {
    await assert.rejects(
      execute(
        {
          wire_version: 1,
          action: "lookup",
          registry_name: "com.example/unsafe-package",
          registry_version: "1.0.0",
        },
        { baseUrl },
      ),
      (error: unknown) => {
        assert.ok(error instanceof RegistryProblem);
        assert.match(error.message, /not safe to expose/u);
        assert.doesNotMatch(error.message, /MUST-NOT-LEAK/u);
        return true;
      },
    );
  });
});

test("HTTP failures preserve the status but discard the untrusted body", async () => {
  await withServer((_request, response) => {
    response.writeHead(429, { "content-type": "text/plain" });
    response.end("token=SHOULD-NOT-LEAK");
  }, async (baseUrl) => {
    await assert.rejects(
      execute(
        { wire_version: 1, action: "search", query: "exa" },
        { baseUrl },
      ),
      (error: unknown) => {
        assert.ok(error instanceof RegistryProblem);
        assert.equal(error.httpStatus, 429);
        assert.match(error.message, /HTTP 429/u);
        assert.doesNotMatch(error.message, /SHOULD-NOT-LEAK/u);
        return true;
      },
    );
  });
});

test("cancellation and timeout are classified from their owning signals", async () => {
  const cancelled = new AbortController();
  cancelled.abort();
  await assert.rejects(
    execute(
      { wire_version: 1, action: "search", query: "exa" },
      { baseUrl: "http://127.0.0.1:1", signal: cancelled.signal },
    ),
    (error: unknown) =>
      error instanceof RegistryProblem && error.kind === "cancelled",
  );

  const timeout = new AbortController();
  timeout.abort();
  await assert.rejects(
    getJson("/v0.1/servers", {
      baseUrl: registryBaseUrl("http://127.0.0.1:1"),
      signal: timeout.signal,
      timeoutSignal: timeout.signal,
    }),
    (error: unknown) =>
      error instanceof RegistryProblem && error.kind === "timeout",
  );
});

test("wire parsing rejects ambiguous requests", () => {
  assert.throws(
    () =>
      parseRequest({ wire_version: 1, action: "search", query: "exa", limit: 1 }),
    /unknown field/u,
  );
  assert.throws(
    () =>
      parseRequest({
        wire_version: 1,
        action: "lookup",
        registry_name: "ai.exa/exa",
        registry_version: "latest",
      }),
    /exact/u,
  );
  assert.throws(
    () => parseRequest({ wire_version: 1, action: "search", query: "exa\nignore" }),
    /control/u,
  );
});

test("the executable returns one terminal record over its real stdio boundary", async () => {
  await withServer((_request, response) => {
    response.writeHead(503, { "content-type": "text/plain" });
    response.end("upstream detail that must not cross the boundary");
  }, async (baseUrl) => {
    const result = await runAdapter(
      { wire_version: 1, action: "search", query: "exa" },
      baseUrl,
    );
    assert.equal(result.code, 0);
    assert.equal(result.stderr, "");
    const record = JSON.parse(result.stdout) as {
      event: string;
      failure: { message: string; diagnostic: { http_status?: number } };
    };
    assert.equal(record.event, "failed");
    assert.equal(record.failure.diagnostic.http_status, 503);
    assert.doesNotMatch(result.stdout, /upstream detail/u);
  });
});

function registryRecord(
  name: string,
  version: string,
  options: {
    readonly description?: string;
    readonly status?: "active" | "deprecated" | "deleted";
    readonly remotes?: readonly unknown[];
    readonly packages?: readonly unknown[];
  } = {},
): unknown {
  return {
    server: {
      name,
      version,
      description: options.description ?? `${name} description`,
      repository: { url: "https://github.com/example/server", source: "github" },
      remotes: options.remotes ?? [],
      packages: options.packages ?? [],
    },
    _meta: {
      "io.modelcontextprotocol.registry/official": {
        status: options.status ?? "active",
        isLatest: true,
      },
    },
  };
}

async function withServer(
  handler: Handler,
  run: (baseUrl: string) => Promise<void>,
): Promise<void> {
  const server = createServer((request, response) => {
    Promise.resolve(handler(request, response)).catch((error: unknown) => {
      response.destroy(error instanceof Error ? error : new Error(String(error)));
    });
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert.ok(address !== null && typeof address !== "string");
  try {
    await run(`http://127.0.0.1:${address.port}`);
  } finally {
    server.close();
    await once(server, "close");
  }
}

function requestUrl(request: IncomingMessage): URL {
  return new URL(request.url ?? "/", "http://127.0.0.1");
}

function respondJson(response: ServerResponse, value: unknown): void {
  const body = JSON.stringify(value);
  response.writeHead(200, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body),
  });
  response.end(body);
}

async function runAdapter(
  request: unknown,
  baseUrl: string,
): Promise<{ readonly code: number | null; readonly stdout: string; readonly stderr: string }> {
  const entry = fileURLToPath(new URL("../src/main.js", import.meta.url));
  const child = spawn(process.execPath, [entry], {
    env: { ...process.env, RENOA_MCP_REGISTRY_BASE_URL: baseUrl },
    stdio: ["pipe", "pipe", "pipe"],
  });
  child.stdin.end(JSON.stringify(request));
  const stdout: Buffer[] = [];
  const stderr: Buffer[] = [];
  child.stdout.on("data", (chunk: Buffer) => stdout.push(chunk));
  child.stderr.on("data", (chunk: Buffer) => stderr.push(chunk));
  const [code] = (await once(child, "close")) as [number | null];
  return {
    code,
    stdout: Buffer.concat(stdout).toString("utf8"),
    stderr: Buffer.concat(stderr).toString("utf8"),
  };
}

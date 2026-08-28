import assert from "node:assert/strict";
import { once } from "node:events";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";

import { execute } from "../src/client.js";
import { parseRequest } from "../src/wire.js";

const DESCRIPTION = "Exa gives agents current web search.";

test("search returns a verified public MCP candidate and resolve rejects drift", async (context) => {
  let endpointSuffix = "";
  const fixture = await catalogFixture(() => endpointSuffix);
  context.after(() => fixture.close());

  const searched = await execute(
    { wire_version: 1, action: "search", query: "exa web search" },
    { baseUrl: fixture.baseUrl },
  );
  assert.equal(searched.event, "completed");
  assert.equal(searched.result.action, "search");
  if (searched.result.action !== "search") {
    assert.fail("expected search result");
  }
  assert.equal(searched.result.candidates.length, 1);
  const candidate = searched.result.candidates[0];
  assert.ok(candidate);
  assert.equal(candidate.endpoint, "https://mcp.exa.ai/mcp");
  assert.deepEqual(candidate.auth, { status: "none" });
  assert.match(candidate.reference, /^integrations\.sh\/exa\.ai\/exa-mcp-server\/[a-f0-9]{64}$/u);

  const resolved = await execute(
    { wire_version: 1, action: "resolve", candidate: candidate.reference },
    { baseUrl: fixture.baseUrl },
  );
  assert.equal(resolved.event, "completed");
  assert.equal(resolved.result.action, "resolve");

  endpointSuffix = "?changed=true";
  await assert.rejects(
    execute(
      { wire_version: 1, action: "resolve", candidate: candidate.reference },
      { baseUrl: fixture.baseUrl },
    ),
    /candidate changed after discovery/u,
  );
});

test("required authentication is visible and cannot be mistaken for public", async (context) => {
  const fixture = await catalogFixture(() => "", true);
  context.after(() => fixture.close());
  const record = await execute(
    { wire_version: 1, action: "search", query: "exa" },
    { baseUrl: fixture.baseUrl },
  );
  assert.equal(record.event, "completed");
  if (record.event !== "completed" || record.result.action !== "search") {
    assert.fail("expected search result");
  }
  assert.deepEqual(record.result.candidates[0]?.auth, {
    status: "required",
    setup: "Create an API key in the provider dashboard.",
    blocker: "This MCP requires credential setup that Renoa cannot provision through catalog discovery yet. Do not request a secret in chat; explain the setup requirement or use an already stored Secret Service bearer credential through the expert connect action.",
  });
});

test("the process wire returns a terminal candidate record", async (context) => {
  const fixture = await catalogFixture(() => "");
  context.after(() => fixture.close());
  const main = fileURLToPath(new URL("../src/main.js", import.meta.url));
  const child = spawn(process.execPath, [main], {
    env: { ...process.env, RENOA_INTEGRATIONS_BASE_URL: fixture.baseUrl },
    stdio: ["pipe", "pipe", "pipe"],
  });
  child.stdin.end(JSON.stringify({ wire_version: 1, action: "search", query: "exa" }));
  const stdout: Buffer[] = [];
  const stderr: Buffer[] = [];
  child.stdout.on("data", (chunk: Buffer) => stdout.push(chunk));
  child.stderr.on("data", (chunk: Buffer) => stderr.push(chunk));
  const [status] = (await once(child, "exit")) as [number | null];
  assert.equal(status, 0, Buffer.concat(stderr).toString("utf8"));
  const record = JSON.parse(Buffer.concat(stdout).toString("utf8")) as {
    event: string;
    result: { candidates: unknown[] };
  };
  assert.equal(record.event, "completed");
  assert.equal(record.result.candidates.length, 1);
});

test("wire rejects unknown fields and malformed candidate references", () => {
  assert.throws(
    () => parseRequest({ wire_version: 1, action: "search", query: "exa", endpoint: "no" }),
    /unknown field/u,
  );
  assert.throws(
    () => parseRequest({ wire_version: 1, action: "resolve", candidate: "exa" }),
    /malformed/u,
  );
});

async function catalogFixture(
  suffix: () => string,
  authenticationRequired = false,
): Promise<{ readonly baseUrl: string; readonly close: () => void }> {
  const server = createServer((request, response) => {
    if (request.url?.startsWith("/api/search?") === true) {
      const query = new URL(request.url, "http://fixture.invalid").searchParams.get("q");
      json(response, {
        results: query?.includes(" ") === true ? [] : [
          {
            domain: "context7.com",
            name: "context7.com",
            description: "An unrelated fuzzy match.",
            kinds: ["mcp"],
            url: "https://integrations.sh/context7.com/",
          },
          {
            domain: "exa.ai",
            name: "exa.ai",
            description: DESCRIPTION,
            kinds: ["mcp"],
            url: "https://integrations.sh/exa.ai/",
          },
        ],
      });
      return;
    }
    if (request.url === "/api/exa.ai/surface") {
      json(response, surfaceRecord(suffix(), authenticationRequired));
      return;
    }
    response.writeHead(404).end();
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  if (address === null || typeof address === "string") {
    throw new Error("fixture has no TCP address");
  }
  return {
    baseUrl: `http://127.0.0.1:${address.port}`,
    close: () => server.close(),
  };
}

function surfaceRecord(suffix: string, authenticationRequired: boolean): unknown {
  return {
    version: 3,
    domain: "exa.ai",
    description: DESCRIPTION,
    credentials: authenticationRequired
      ? { exa_api_key: { setup: "Create an API key in the provider dashboard." } }
      : {},
    surfaces: [
      {
        type: "mcp",
        url: `https://mcp.exa.ai/mcp${suffix}`,
        transports: ["streamable-http"],
        slug: "exa-mcp-server",
        name: "Exa MCP Server",
        docs: "https://exa.ai/docs/reference/exa-mcp",
        basis: {
          via: "discovered",
          evidence: ["https://exa.ai/mcp", "https://exa.ai/docs/reference/exa-mcp"],
        },
        auth: authenticationRequired ? { status: "required" } : { status: "none" },
      },
    ],
  };
}

function json(response: import("node:http").ServerResponse, value: unknown): void {
  const encoded = JSON.stringify(value);
  response.writeHead(200, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(encoded),
  });
  response.end(encoded);
}

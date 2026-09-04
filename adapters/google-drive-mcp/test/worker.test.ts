import { env } from "cloudflare:workers";
import {
  createExecutionContext,
  waitOnExecutionContext,
} from "cloudflare:test";
import {
  Client,
  SUPPORTED_PROTOCOL_VERSIONS,
  StreamableHTTPClientTransport,
  type CallToolResult,
  type FetchLike,
} from "@modelcontextprotocol/client";
import { afterEach, describe, expect, test } from "vitest";

import {
  GOOGLE_DRIVE_SCOPE,
  MCP_ENDPOINT,
  RESOURCE_METADATA_PATH,
} from "../src/constants.js";
import type { DriveFetch } from "../src/drive-client.js";
import { createWorker, type DriveWorker } from "../src/index.js";
import { isJsonObject } from "../src/json.js";

const ACCESS_TOKEN = "test-google-access-token";
const clients: Client[] = [];

afterEach(async () => {
  await Promise.all(clients.splice(0).map((client) => client.close()));
});

describe("OAuth boundary", () => {
  test("publishes exact protected-resource metadata", async () => {
    const worker = createWorker(unusedGoogleFetch);
    const response = await dispatch(
      worker,
      new Request(`https://drive.renoa.live${RESOURCE_METADATA_PATH}`),
    );

    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({
      resource: MCP_ENDPOINT,
      authorization_servers: ["https://accounts.google.com"],
      scopes_supported: [
        GOOGLE_DRIVE_SCOPE,
        "https://www.googleapis.com/auth/drive.readonly",
        "https://www.googleapis.com/auth/drive.file",
      ],
      bearer_methods_supported: ["header"],
      resource_name: "Renoa Google Drive",
    });
  });

  test("rejects missing and malformed bearer credentials before MCP", async () => {
    const worker = createWorker(unusedGoogleFetch);
    for (const authorization of [undefined, "Basic abc", "Bearer a b"]) {
      const headers = authorization === undefined ? {} : { Authorization: authorization };
      const response = await dispatch(
        worker,
        new Request(MCP_ENDPOINT, { method: "POST", headers }),
      );
      expect(response.status).toBe(401);
      expect(response.headers.get("www-authenticate")).toBe(
        `Bearer resource_metadata="https://drive.renoa.live${RESOURCE_METADATA_PATH}", scope="${GOOGLE_DRIVE_SCOPE}"`,
      );
    }
  });
});

describe("MCP tools", () => {
  test("advertises exactly the eight focused Drive tools", async () => {
    const { client, googleRequests } = await connectedClient(async () => {
      throw new Error("tool discovery must not call Google");
    });
    const result = await client.listTools();

    expect(result.tools.map((tool) => tool.name).sort()).toEqual([
      "copy_file",
      "create_file",
      "download_file_content",
      "get_file_metadata",
      "get_file_permissions",
      "list_recent_files",
      "read_file_content",
      "search_files",
    ]);
    expect(googleRequests).toHaveLength(0);
  });

  test("search sends one scoped Drive request and preserves the result", async () => {
    const { client, googleRequests } = await connectedClient(async (request) => {
      expect(request.headers.get("authorization")).toBe(`Bearer ${ACCESS_TOKEN}`);
      return Response.json({
        files: [{ id: "file_1", name: "Roadmap", mimeType: "text/plain" }],
        nextPageToken: "next-page",
      });
    });

    const result = await client.callTool({
      name: "search_files",
      arguments: {
        query: "name contains 'Roadmap'",
        orderBy: "modifiedTime desc",
        pageSize: 25,
      },
    });

    expect(result.isError).not.toBe(true);
    expect(toolJson(result)).toMatchObject({
      files: [{ id: "file_1", name: "Roadmap" }],
      nextPageToken: "next-page",
    });
    expect(googleRequests).toHaveLength(1);
    const url = new URL(googleRequests[0]!.url);
    expect(url.pathname).toBe("/drive/v3/files");
    expect(url.searchParams.get("q")).toBe(
      "trashed = false and (name contains 'Roadmap')",
    );
    expect(url.searchParams.get("pageSize")).toBe("25");
    expect(url.searchParams.get("includeItemsFromAllDrives")).toBe("true");
    expect(url.searchParams.get("supportsAllDrives")).toBe("true");
  });

  test("reads a Google Doc through export and pages text without cutting code points", async () => {
    const { client, googleRequests } = await connectedClient(async (request) => {
      const url = new URL(request.url);
      if (url.pathname.endsWith("/export")) {
        return new Response("zero😀one😀two", {
          headers: { "Content-Type": "text/plain; charset=utf-8" },
        });
      }
      return Response.json({
        id: "doc_1",
        name: "Notes",
        mimeType: "application/vnd.google-apps.document",
      });
    });

    const result = await client.callTool({
      name: "read_file_content",
      arguments: { fileId: "doc_1", startCharacter: 4, maxCharacters: 5 },
    });

    expect(toolJson(result)).toMatchObject({
      contentMimeType: "text/plain",
      startCharacter: 4,
      content: "😀one😀",
      complete: false,
      nextCharacter: 9,
    });
    expect(googleRequests).toHaveLength(2);
    expect(new URL(googleRequests[1]!.url).searchParams.get("mimeType")).toBe("text/plain");
  });

  test("uploads text with multipart metadata and never puts the token in the body", async () => {
    const { client, googleRequests } = await connectedClient(async () =>
      Response.json({
        id: "created_1",
        name: "Daily note",
        mimeType: "application/vnd.google-apps.document",
      }),
    );

    const result = await client.callTool({
      name: "create_file",
      arguments: {
        title: "Daily note",
        textContent: "Today went well.",
        contentMimeType: "text/plain",
        driveMimeType: "application/vnd.google-apps.document",
      },
    });

    expect(result.isError).not.toBe(true);
    expect(toolJson(result)).toMatchObject({ id: "created_1", name: "Daily note" });
    expect(googleRequests).toHaveLength(1);
    const request = googleRequests[0]!;
    expect(new URL(request.url).pathname).toBe("/upload/drive/v3/files");
    expect(new URL(request.url).searchParams.get("uploadType")).toBe("multipart");
    const body = new TextDecoder().decode(await request.arrayBuffer());
    expect(body).toContain('"name":"Daily note"');
    expect(body).toContain('"mimeType":"application/vnd.google-apps.document"');
    expect(body).toContain("Today went well.");
    expect(body).not.toContain(ACCESS_TOKEN);
  });

  test("supports byte ranges, copies, and permission reads", async () => {
    const { client, googleRequests } = await connectedClient(async (request) => {
      const url = new URL(request.url);
      if (url.searchParams.get("alt") === "media") {
        expect(request.headers.get("range")).toBe("bytes=2-5");
        return new Response(new Uint8Array([2, 3, 4, 5]), {
          status: 206,
          headers: {
            "Content-Type": "application/octet-stream",
            "Content-Range": "bytes 2-5/8",
          },
        });
      }
      if (url.pathname.endsWith("/copy")) {
        return Response.json({ id: "copy_1", name: "Copy" });
      }
      if (url.pathname.endsWith("/permissions")) {
        return Response.json({ permissions: [{ id: "owner", type: "user", role: "owner" }] });
      }
      return Response.json({ id: "binary_1", mimeType: "application/octet-stream" });
    });

    const downloaded = await client.callTool({
      name: "download_file_content",
      arguments: { fileId: "binary_1", byteOffset: 2, maxBytes: 4 },
    });
    expect(toolJson(downloaded)).toMatchObject({
      base64Content: "AgMEBQ==",
      byteOffset: 2,
      returnedBytes: 4,
      complete: false,
      nextByteOffset: 6,
    });

    const copied = await client.callTool({
      name: "copy_file",
      arguments: { fileId: "binary_1", title: "Copy" },
    });
    expect(toolJson(copied)).toMatchObject({ id: "copy_1" });

    const permissions = await client.callTool({
      name: "get_file_permissions",
      arguments: { fileId: "binary_1" },
    });
    expect(toolJson(permissions)).toMatchObject({
      permissions: [{ id: "owner", role: "owner" }],
    });
    expect(googleRequests).toHaveLength(4);
  });

  test("returns Google errors as definite tool results without leaking credentials", async () => {
    const { client } = await connectedClient(async () =>
      Response.json(
        {
          error: {
            code: 403,
            message: "The caller does not have permission.",
            errors: [{ reason: `insufficient-${ACCESS_TOKEN}` }],
          },
        },
        { status: 403 },
      ),
    );

    const result = await client.callTool({
      name: "list_recent_files",
      arguments: {},
    });

    expect(result.isError).toBe(true);
    expect(toolJson(result)).toEqual({
      error: "The caller does not have permission.",
      status: 403,
      reason: "insufficient-[REDACTED]",
    });
    expect(textContent(result)).not.toContain(ACCESS_TOKEN);
  });

  test("rejects invalid tool arguments before any Google request", async () => {
    const { client, googleRequests } = await connectedClient(async () => {
      throw new Error("invalid arguments must not reach Google");
    });

    const result = await client.callTool({
      name: "search_files",
      arguments: { query: "   " },
    });
    expect(result.isError).toBe(true);
    expect(textContent(result)).toContain("Input validation error");
    expect(googleRequests).toHaveLength(0);
  });

  test("refuses Google redirects instead of forwarding the bearer token", async () => {
    const { client, googleRequests } = await connectedClient(async () =>
      new Response(null, {
        status: 302,
        headers: { Location: "https://example.com/capture" },
      }),
    );

    const result = await client.callTool({
      name: "list_recent_files",
      arguments: {},
    });

    expect(result.isError).toBe(true);
    expect(toolJson(result)).toEqual({
      error: "Google Drive returned an unexpected redirect.",
      status: 502,
    });
    expect(googleRequests).toHaveLength(1);
    expect(new URL(googleRequests[0]!.url).origin).toBe("https://www.googleapis.com");
  });
});

async function connectedClient(
  googleHandler: (request: RecordedRequest) => Promise<Response>,
): Promise<{ client: Client; googleRequests: RecordedRequest[] }> {
  const googleRequests: RecordedRequest[] = [];
  const googleFetch: DriveFetch = async (input, init) => {
    const request = new Request(input, init);
    googleRequests.push(request.clone());
    return googleHandler(request);
  };
  const worker = createWorker(googleFetch);
  const transport = new StreamableHTTPClientTransport(new URL(MCP_ENDPOINT), {
    fetch: workerFetch(worker),
    requestInit: { headers: { Authorization: `Bearer ${ACCESS_TOKEN}` } },
    reconnectionOptions: {
      maxReconnectionDelay: 1,
      initialReconnectionDelay: 1,
      reconnectionDelayGrowFactor: 1,
      maxRetries: 0,
    },
  });
  const client = new Client(
    { name: "renoa-drive-test", version: "0.1.0" },
    {
      capabilities: {},
      supportedProtocolVersions: [...SUPPORTED_PROTOCOL_VERSIONS],
      enforceStrictCapabilities: true,
      versionNegotiation: { mode: "auto", probe: { maxRetries: 0 } },
      inputRequired: { autoFulfill: false },
    },
  );
  clients.push(client);
  await client.connect(transport);
  return { client, googleRequests };
}

interface RecordedRequest {
  readonly url: string;
  readonly headers: Headers;
  arrayBuffer(): Promise<ArrayBuffer>;
}

function workerFetch(worker: DriveWorker): FetchLike {
  return async (input, init) => {
    const headers = new Headers(init?.headers);
    const url = new URL(input instanceof Request ? input.url : input);
    headers.set("Host", url.host);
    const request = new Request(input, { ...init, headers });
    return dispatch(worker, request);
  };
}

async function dispatch(worker: DriveWorker, request: Request): Promise<Response> {
  const context = createExecutionContext();
  const response = await worker.fetch(request, env, context);
  await waitOnExecutionContext(context);
  return response;
}

const unusedGoogleFetch: DriveFetch = async () => {
  throw new Error("Google fetch was not expected");
};

function toolJson(result: CallToolResult): Record<string, unknown> {
  const value: unknown = JSON.parse(textContent(result));
  if (!isJsonObject(value)) {
    throw new Error("expected tool text to contain one JSON object");
  }
  return value;
}

function textContent(result: CallToolResult): string {
  const block = result.content[0];
  if (block?.type !== "text") {
    throw new Error("expected one text content block");
  }
  return block.text;
}

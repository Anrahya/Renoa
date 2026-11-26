import { env } from "cloudflare:workers";
import { createExecutionContext, waitOnExecutionContext } from "cloudflare:test";
import {
  Client,
  SUPPORTED_PROTOCOL_VERSIONS,
  StreamableHTTPClientTransport,
  type CallToolResult,
  type FetchLike,
} from "@modelcontextprotocol/client";
import { afterEach, describe, expect, test } from "vitest";

import {
  GOOGLE_GMAIL_COMPOSE_SCOPE,
  GOOGLE_GMAIL_MODIFY_SCOPE,
  GOOGLE_GMAIL_READONLY_SCOPE,
  GOOGLE_GMAIL_SEND_SCOPE,
  MCP_ENDPOINT,
  RESOURCE_METADATA_PATH,
} from "../src/constants.js";
import { createWorker, type GmailWorker } from "../src/index.js";
import { isJsonObject } from "../src/json.js";
import { base64UrlDecode, base64UrlEncode } from "../src/validation.js";

const ACCESS_TOKEN = "test-google-gmail-token";
const clients: Client[] = [];

afterEach(async () => {
  await Promise.all(clients.splice(0).map((client) => client.close()));
});

describe("OAuth boundary", () => {
  test("publishes exact protected-resource metadata", async () => {
    const worker = createWorker(unusedGoogleFetch);
    const response = await dispatch(
      worker,
      new Request(`https://gmail.renoa.live${RESOURCE_METADATA_PATH}`),
    );
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({
      resource: MCP_ENDPOINT,
      authorization_servers: ["https://accounts.google.com"],
      scopes_supported: [
        GOOGLE_GMAIL_MODIFY_SCOPE,
        GOOGLE_GMAIL_READONLY_SCOPE,
        GOOGLE_GMAIL_COMPOSE_SCOPE,
        GOOGLE_GMAIL_SEND_SCOPE,
      ],
      bearer_methods_supported: ["header"],
      resource_name: "Renoa Google Gmail",
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
        `Bearer resource_metadata="https://gmail.renoa.live${RESOURCE_METADATA_PATH}", scope="${GOOGLE_GMAIL_MODIFY_SCOPE}"`,
      );
    }
  });
});

describe("MCP tools", () => {
  test("advertises the ten focused Gmail tools", async () => {
    const { client, googleRequests } = await connectedClient(async () => {
      throw new Error("tool discovery must not call Google");
    });
    const result = await client.listTools();
    expect(result.tools.map((tool) => tool.name).sort()).toEqual([
      "create_draft",
      "get_message",
      "get_thread",
      "list_drafts",
      "list_labels",
      "modify_thread_labels",
      "search_threads",
      "send_draft",
      "send_message",
      "trash_thread",
    ]);
    expect(googleRequests).toHaveLength(0);
  });

  test("search_threads hydrates metadata headers and get_thread decodes MIME text", async () => {
    const bodyBase64 = base64UrlEncode(new TextEncoder().encode("Hello from Alice!"));
    const { client, googleRequests } = await connectedClient(async (request) => {
      expect(request.headers.get("authorization")).toBe(`Bearer ${ACCESS_TOKEN}`);
      const url = new URL(request.url);
      if (url.pathname === "/gmail/v1/users/me/threads") {
        return Response.json({
          threads: [{ id: "thread_1", snippet: "Hello from Alice!" }],
          resultSizeEstimate: 1,
        });
      }
      if (url.pathname === "/gmail/v1/users/me/threads/thread_1") {
        return Response.json({
          id: "thread_1",
          historyId: "99",
          messages: [
            {
              id: "msg_1",
              threadId: "thread_1",
              internalDate: "1795700000000",
              labelIds: ["INBOX", "UNREAD"],
              snippet: "Hello from Alice!",
              payload: {
                mimeType: "multipart/alternative",
                headers: [
                  { name: "From", value: "Alice <alice@example.com>" },
                  { name: "To", value: "me@example.com" },
                  { name: "Subject", value: "Lunch?" },
                  { name: "Date", value: "Thu, 26 Nov 2026 12:00:00 +0000" },
                ],
                parts: [
                  { mimeType: "text/plain", body: { data: bodyBase64 } },
                  {
                    mimeType: "application/pdf",
                    filename: "menu.pdf",
                    body: { attachmentId: "att_1", size: 4096 },
                  },
                ],
              },
            },
          ],
        });
      }
      throw new Error(`unexpected request: ${request.url}`);
    });

    const searched = await client.callTool({
      name: "search_threads",
      arguments: { query: "is:unread", maxResults: 10 },
    });
    expect(searched.isError).not.toBe(true);
    expect(toolJson(searched)).toMatchObject({
      resultSizeEstimate: 1,
      threads: [
        {
          id: "thread_1",
          messageCount: 1,
          labelIds: ["INBOX", "UNREAD"],
          headers: { from: "Alice <alice@example.com>", subject: "Lunch?" },
        },
      ],
    });

    const thread = await client.callTool({
      name: "get_thread",
      arguments: { threadId: "thread_1" },
    });
    expect(thread.isError).not.toBe(true);
    expect(toolJson(thread)).toMatchObject({
      id: "thread_1",
      messageCount: 1,
      messages: [
        {
          id: "msg_1",
          body: "Hello from Alice!",
          truncated: false,
          attachments: [{ filename: "menu.pdf", mimeType: "application/pdf", size: 4096 }],
        },
      ],
    });
    expect(googleRequests).toHaveLength(3);
  });

  test("create_draft encodes RFC 2822 raw message and rejects header injection", async () => {
    let sentRaw = "";
    const { client } = await connectedClient(async (request) => {
      const payload = (await request.json()) as { message: { raw: string; threadId?: string } };
      sentRaw = new TextDecoder().decode(base64UrlDecode(payload.message.raw));
      return Response.json({ id: "draft_1", message: { id: "msg_2", threadId: "thread_1" } });
    });

    const created = await client.callTool({
      name: "create_draft",
      arguments: {
        to: "alice@example.com",
        subject: "Re: Lunch?",
        body: "Sounds great!",
        threadId: "thread_1",
      },
    });
    expect(created.isError).not.toBe(true);
    expect(sentRaw).toContain("To: alice@example.com\r\n");
    expect(sentRaw).toContain("Subject: Re: Lunch?\r\n");
    expect(sentRaw).toContain("\r\n\r\nSounds great!");

    const injected = await client.callTool({
      name: "create_draft",
      arguments: {
        to: "alice@example.com\r\nBcc: evil@example.com",
        subject: "Hi",
        body: "Nope",
      },
    });
    expect(injected.isError).toBe(true);
  });

  test("redacts bearer token from upstream errors", async () => {
    const { client } = await connectedClient(async () =>
      Response.json(
        { error: { message: `Invalid token ${ACCESS_TOKEN}` } },
        { status: 401 },
      ),
    );
    const result = await client.callTool({ name: "list_labels", arguments: {} });
    expect(result.isError).toBe(true);
    const body = toolJson(result);
    expect(body.error).toBe("Invalid token [REDACTED]");
  });
});

async function connectedClient(handler: (request: Request) => Promise<Response>): Promise<{
  client: Client;
  googleRequests: Request[];
}> {
  const googleRequests: Request[] = [];
  const worker = createWorker(async (input, init) => {
    const request = input instanceof Request ? input : new Request(input, init);
    googleRequests.push(request);
    return handler(request);
  });
  const fetchLike: FetchLike = (input, init) =>
    dispatch(worker, input instanceof Request ? input : new Request(input, init));
  const transport = new StreamableHTTPClientTransport(new URL(MCP_ENDPOINT), {
    fetch: fetchLike,
    requestInit: { headers: { Authorization: `Bearer ${ACCESS_TOKEN}` } },
  });
  const client = new Client(
    { name: "renoa-gmail-test", version: "0.1.0" },
    { supportedProtocolVersions: [...SUPPORTED_PROTOCOL_VERSIONS] },
  );
  await client.connect(transport);
  clients.push(client);
  return { client, googleRequests };
}

async function dispatch(worker: GmailWorker, request: Request): Promise<Response> {
  const headers = new Headers(request.headers);
  if (!headers.has("host")) {
    headers.set("host", new URL(request.url).host);
  }
  const ctx = createExecutionContext();
  const response = await worker.fetch(new Request(request, { headers }), env, ctx);
  await waitOnExecutionContext(ctx);
  return response;
}

function unusedGoogleFetch(): Promise<Response> {
  return Promise.reject(new Error("Google should not have been called"));
}

function toolJson(result: Awaited<ReturnType<Client["callTool"]>>): Record<string, unknown> {
  const call = result as CallToolResult;
  expect(isJsonObject(call.structuredContent)).toBe(true);
  return call.structuredContent as Record<string, unknown>;
}

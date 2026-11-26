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
  GOOGLE_CALENDAR_EVENTS_SCOPE,
  GOOGLE_CALENDAR_READONLY_SCOPE,
  GOOGLE_CALENDAR_SCOPE,
  MCP_ENDPOINT,
  RESOURCE_METADATA_PATH,
} from "../src/constants.js";
import { createWorker, type CalendarWorker } from "../src/index.js";
import { isJsonObject } from "../src/json.js";

const ACCESS_TOKEN = "test-google-calendar-token";
const clients: Client[] = [];

afterEach(async () => {
  await Promise.all(clients.splice(0).map((client) => client.close()));
});

describe("OAuth boundary", () => {
  test("publishes exact protected-resource metadata", async () => {
    const worker = createWorker(unusedGoogleFetch);
    const response = await dispatch(
      worker,
      new Request(`https://calendar.renoa.live${RESOURCE_METADATA_PATH}`),
    );
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({
      resource: MCP_ENDPOINT,
      authorization_servers: ["https://accounts.google.com"],
      scopes_supported: [
        GOOGLE_CALENDAR_SCOPE,
        GOOGLE_CALENDAR_READONLY_SCOPE,
        GOOGLE_CALENDAR_EVENTS_SCOPE,
      ],
      bearer_methods_supported: ["header"],
      resource_name: "Renoa Google Calendar",
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
        `Bearer resource_metadata="https://calendar.renoa.live${RESOURCE_METADATA_PATH}", scope="${GOOGLE_CALENDAR_SCOPE}"`,
      );
    }
  });
});

describe("MCP tools", () => {
  test("advertises the seven focused Calendar tools", async () => {
    const { client, googleRequests } = await connectedClient(async () => {
      throw new Error("tool discovery must not call Google");
    });
    const result = await client.listTools();
    expect(result.tools.map((tool) => tool.name).sort()).toEqual([
      "create_event",
      "delete_event",
      "get_event",
      "list_calendars",
      "list_events",
      "query_freebusy",
      "update_event",
    ]);
    expect(googleRequests).toHaveLength(0);
  });

  test("list_events, create_event, and delete_event hit Calendar v3 endpoints cleanly", async () => {
    const { client, googleRequests } = await connectedClient(async (request) => {
      expect(request.headers.get("authorization")).toBe(`Bearer ${ACCESS_TOKEN}`);
      const url = new URL(request.url);
      if (request.method === "GET" && url.pathname === "/calendar/v3/calendars/primary/events") {
        return Response.json({ items: [{ id: "evt_1", summary: "Standup" }] });
      }
      if (request.method === "POST" && url.pathname === "/calendar/v3/calendars/primary/events") {
        const body = (await request.json()) as Record<string, unknown>;
        return Response.json({ id: "evt_2", ...body });
      }
      if (request.method === "DELETE" && url.pathname === "/calendar/v3/calendars/primary/events/evt_2") {
        return new Response(null, { status: 204 });
      }
      throw new Error(`unexpected request: ${request.method} ${request.url}`);
    });

    const listed = await client.callTool({
      name: "list_events",
      arguments: {
        timeMin: "2026-11-26T00:00:00+05:30",
        timeMax: "2026-11-27T00:00:00+05:30",
      },
    });
    expect(listed.isError).not.toBe(true);
    expect(toolJson(listed)).toMatchObject({ items: [{ id: "evt_1", summary: "Standup" }] });

    const created = await client.callTool({
      name: "create_event",
      arguments: {
        summary: "Gym",
        start: "2026-11-26T07:00:00+05:30",
        end: "2026-11-26T08:00:00+05:30",
      },
    });
    expect(created.isError).not.toBe(true);
    expect(toolJson(created)).toMatchObject({ id: "evt_2", summary: "Gym" });

    const deleted = await client.callTool({
      name: "delete_event",
      arguments: { eventId: "evt_2" },
    });
    expect(deleted.isError).not.toBe(true);
    expect(toolJson(deleted)).toEqual({ deleted: true, calendarId: "primary", eventId: "evt_2" });
    expect(googleRequests).toHaveLength(3);
  });

  test("rejects malformed timestamps and redacts token from errors", async () => {
    const { client } = await connectedClient(async () =>
      Response.json(
        { error: { message: `Denied for ${ACCESS_TOKEN}` } },
        { status: 403 },
      ),
    );

    const invalid = await client.callTool({
      name: "list_events",
      arguments: { timeMin: "not-a-timestamp" },
    });
    expect(invalid.isError).toBe(true);

    const failed = await client.callTool({ name: "list_calendars", arguments: {} });
    expect(failed.isError).toBe(true);
    expect(toolJson(failed).error).toBe("Denied for [REDACTED]");
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
    { name: "renoa-calendar-test", version: "0.1.0" },
    { supportedProtocolVersions: [...SUPPORTED_PROTOCOL_VERSIONS] },
  );
  await client.connect(transport);
  clients.push(client);
  return { client, googleRequests };
}

async function dispatch(worker: CalendarWorker, request: Request): Promise<Response> {
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

import { McpServer, type McpRequestContext } from "@modelcontextprotocol/server";
import { z } from "zod";

import { CalendarClient, type CalendarFetch } from "./calendar-client.js";
import { publicToolError } from "./errors.js";
import type { JsonObject } from "./json.js";

type ToolResult = {
  content: [{ type: "text"; text: string }];
  structuredContent: JsonObject;
  isError?: boolean;
};

const readOnly = {
  readOnlyHint: true,
  destructiveHint: false,
  idempotentHint: true,
  openWorldHint: true,
} as const;

const mutating = {
  readOnlyHint: false,
  destructiveHint: false,
  idempotentHint: false,
  openWorldHint: true,
} as const;

const destructive = {
  readOnlyHint: false,
  destructiveHint: true,
  idempotentHint: false,
  openWorldHint: true,
} as const;

const calendarIdSchema = z.string().min(1).max(256).default("primary");
const eventIdSchema = z.string().min(1).max(1_024);
const pageTokenSchema = z.string().min(1).max(4_096).optional();
const sendUpdatesSchema = z.enum(["all", "externalOnly", "none"]).default("none");

export function createCalendarServer(
  context: McpRequestContext,
  fetchFn: CalendarFetch = fetch,
): McpServer {
  const request = context.requestInfo;
  const token = request === undefined ? undefined : bearerToken(request.headers);
  if (request === undefined || token === undefined) {
    throw new Error("Authenticated HTTP request context is required.");
  }
  const calendar = new CalendarClient(token, request.signal, fetchFn);
  const server = new McpServer({ name: "renoa-google-calendar", version: "0.1.0" });

  server.registerTool(
    "list_calendars",
    {
      description: "List the user's Google Calendars with their exact IDs, names, and time zones.",
      inputSchema: z.object({
        maxResults: z.number().int().min(1).max(100).default(50),
        pageToken: pageTokenSchema,
      }),
      annotations: readOnly,
    },
    ({ maxResults, pageToken }) => run(token, () => calendar.listCalendars(maxResults, pageToken)),
  );

  server.registerTool(
    "list_events",
    {
      description:
        "List or search events on a Google Calendar. Pass RFC3339 timestamps with timezone offset for timeMin/timeMax.",
      inputSchema: z.object({
        calendarId: calendarIdSchema,
        timeMin: z.string().min(1).max(64).optional().describe("Inclusive RFC3339 lower bound."),
        timeMax: z.string().min(1).max(64).optional().describe("Exclusive RFC3339 upper bound."),
        query: z.string().max(1_024).optional().describe("Free-text search term."),
        singleEvents: z.boolean().default(true).describe("Expand recurring events into individual instances."),
        orderBy: z.enum(["startTime", "updated"]).default("startTime"),
        maxResults: z.number().int().min(1).max(100).default(25),
        pageToken: pageTokenSchema,
      }),
      annotations: readOnly,
    },
    (args) =>
      run(token, () =>
        calendar.listEvents({
          calendarId: args.calendarId,
          ...(args.timeMin === undefined ? {} : { timeMin: args.timeMin }),
          ...(args.timeMax === undefined ? {} : { timeMax: args.timeMax }),
          ...(args.query === undefined ? {} : { query: args.query }),
          singleEvents: args.singleEvents,
          orderBy: args.orderBy,
          maxResults: args.maxResults,
          ...(args.pageToken === undefined ? {} : { pageToken: args.pageToken }),
        }),
      ),
  );

  server.registerTool(
    "get_event",
    {
      description: "Fetch one Google Calendar event by calendarId and eventId.",
      inputSchema: z.object({
        calendarId: calendarIdSchema,
        eventId: eventIdSchema,
      }),
      annotations: readOnly,
    },
    ({ calendarId, eventId }) => run(token, () => calendar.getEvent(calendarId, eventId)),
  );

  server.registerTool(
    "create_event",
    {
      description:
        "Create a Google Calendar event. Pass RFC3339 timestamps (or YYYY-MM-DD when allDay is true) for start and end.",
      inputSchema: z.object({
        calendarId: calendarIdSchema,
        summary: z.string().min(1).max(1_024),
        start: z.string().min(1).max(64),
        end: z.string().min(1).max(64),
        allDay: z.boolean().default(false),
        timeZone: z.string().min(1).max(128).optional(),
        description: z.string().max(32 * 1024).optional(),
        location: z.string().max(1_024).optional(),
        attendees: z.array(z.string().min(3).max(256)).max(100).optional(),
        sendUpdates: sendUpdatesSchema,
      }),
      annotations: mutating,
    },
    (args) =>
      run(token, () =>
        calendar.createEvent({
          calendarId: args.calendarId,
          summary: args.summary,
          start: args.start,
          end: args.end,
          allDay: args.allDay,
          ...(args.timeZone === undefined ? {} : { timeZone: args.timeZone }),
          ...(args.description === undefined ? {} : { description: args.description }),
          ...(args.location === undefined ? {} : { location: args.location }),
          ...(args.attendees === undefined ? {} : { attendees: args.attendees }),
          sendUpdates: args.sendUpdates,
        }),
      ),
  );

  server.registerTool(
    "update_event",
    {
      description: "Patch fields on an existing Google Calendar event.",
      inputSchema: z.object({
        calendarId: calendarIdSchema,
        eventId: eventIdSchema,
        summary: z.string().min(1).max(1_024).optional(),
        start: z.string().min(1).max(64).optional(),
        end: z.string().min(1).max(64).optional(),
        allDay: z.boolean().default(false),
        timeZone: z.string().min(1).max(128).optional(),
        description: z.string().max(32 * 1024).optional(),
        location: z.string().max(1_024).optional(),
        attendees: z.array(z.string().min(3).max(256)).max(100).optional(),
        sendUpdates: sendUpdatesSchema,
      }),
      annotations: mutating,
    },
    (args) =>
      run(token, () =>
        calendar.updateEvent({
          calendarId: args.calendarId,
          eventId: args.eventId,
          allDay: args.allDay,
          ...(args.summary === undefined ? {} : { summary: args.summary }),
          ...(args.start === undefined ? {} : { start: args.start }),
          ...(args.end === undefined ? {} : { end: args.end }),
          ...(args.timeZone === undefined ? {} : { timeZone: args.timeZone }),
          ...(args.description === undefined ? {} : { description: args.description }),
          ...(args.location === undefined ? {} : { location: args.location }),
          ...(args.attendees === undefined ? {} : { attendees: args.attendees }),
          sendUpdates: args.sendUpdates,
        }),
      ),
  );

  server.registerTool(
    "delete_event",
    {
      description: "Delete a Google Calendar event.",
      inputSchema: z.object({
        calendarId: calendarIdSchema,
        eventId: eventIdSchema,
        sendUpdates: sendUpdatesSchema,
      }),
      annotations: destructive,
    },
    ({ calendarId, eventId, sendUpdates }) =>
      run(token, () => calendar.deleteEvent(calendarId, eventId, sendUpdates)),
  );

  server.registerTool(
    "query_freebusy",
    {
      description: "Query busy time intervals across one or more calendars within an RFC3339 time window.",
      inputSchema: z.object({
        timeMin: z.string().min(1).max(64),
        timeMax: z.string().min(1).max(64),
        calendarIds: z.array(z.string().min(1).max(256)).min(1).max(20).default(["primary"]),
        timeZone: z.string().min(1).max(128).optional(),
      }),
      annotations: readOnly,
    },
    ({ timeMin, timeMax, calendarIds, timeZone }) =>
      run(token, () => calendar.queryFreeBusy(timeMin, timeMax, calendarIds, timeZone)),
  );

  return server;
}

export function bearerToken(headers: Headers | Record<string, string | string[] | undefined>): string | undefined {
  const raw = headers instanceof Headers
    ? headers.get("authorization")
    : headerFromRecord(headers, "authorization");
  if (raw === null || raw === undefined) return undefined;
  const match = /^Bearer[ \t]+([^\s]+)$/i.exec(raw.trim());
  const token = match?.[1];
  if (token === undefined || token.length === 0 || token.length > 16_384 || /[\u0000-\u001F\u007F]/u.test(token)) {
    return undefined;
  }
  return token;
}

function headerFromRecord(
  headers: Record<string, string | string[] | undefined>,
  name: string,
): string | undefined {
  for (const [key, value] of Object.entries(headers)) {
    if (key.toLowerCase() !== name) continue;
    return Array.isArray(value) ? value[0] : value;
  }
  return undefined;
}

async function run(token: string, action: () => Promise<JsonObject>): Promise<ToolResult> {
  try {
    const result = await action();
    return {
      content: [{ type: "text", text: JSON.stringify(result) }],
      structuredContent: result,
    };
  } catch (error) {
    const failure = publicToolError(error, token);
    return {
      isError: true,
      content: [{ type: "text", text: JSON.stringify(failure) }],
      structuredContent: failure,
    };
  }
}

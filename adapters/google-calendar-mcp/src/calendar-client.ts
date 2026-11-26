import {
  GOOGLE_API_ORIGIN,
  MAX_JSON_RESPONSE_BYTES,
  REQUEST_TIMEOUT_MS,
} from "./constants.js";
import { CalendarApiError } from "./errors.js";
import { type JsonObject, requireJsonObject } from "./json.js";
import { calendarId, eventId, eventTimeBound, rfc3339 } from "./validation.js";

export type CalendarFetch = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => Promise<Response>;

export type SendUpdates = "all" | "externalOnly" | "none";

export interface ListEventsInput {
  readonly calendarId: string;
  readonly timeMin?: string;
  readonly timeMax?: string;
  readonly query?: string;
  readonly singleEvents: boolean;
  readonly orderBy?: "startTime" | "updated";
  readonly maxResults: number;
  readonly pageToken?: string;
}

export interface CreateEventInput {
  readonly calendarId: string;
  readonly summary: string;
  readonly start: string;
  readonly end: string;
  readonly allDay: boolean;
  readonly timeZone?: string;
  readonly description?: string;
  readonly location?: string;
  readonly attendees?: readonly string[];
  readonly sendUpdates: SendUpdates;
}

export interface UpdateEventInput {
  readonly calendarId: string;
  readonly eventId: string;
  readonly summary?: string;
  readonly start?: string;
  readonly end?: string;
  readonly allDay: boolean;
  readonly timeZone?: string;
  readonly description?: string;
  readonly location?: string;
  readonly attendees?: readonly string[];
  readonly sendUpdates: SendUpdates;
}

export class CalendarClient {
  readonly #token: string;
  readonly #signal: AbortSignal;
  readonly #fetch: CalendarFetch;

  constructor(token: string, signal: AbortSignal, fetchFn: CalendarFetch = fetch) {
    this.#token = token;
    this.#signal = signal;
    this.#fetch = fetchFn;
  }

  async listCalendars(maxResults: number, pageToken?: string): Promise<JsonObject> {
    const params = new URLSearchParams({ maxResults: String(maxResults) });
    if (pageToken !== undefined && pageToken.length > 0) {
      params.set("pageToken", pageToken);
    }
    return this.#requestJson(`/calendar/v3/users/me/calendarList?${params}`);
  }

  async listEvents(input: ListEventsInput): Promise<JsonObject> {
    const cal = calendarId(input.calendarId);
    const params = new URLSearchParams({
      maxResults: String(input.maxResults),
      singleEvents: String(input.singleEvents),
    });
    if (input.timeMin !== undefined) {
      params.set("timeMin", rfc3339(input.timeMin, "timeMin"));
    }
    if (input.timeMax !== undefined) {
      params.set("timeMax", rfc3339(input.timeMax, "timeMax"));
    }
    if (input.query !== undefined && input.query.trim().length > 0) {
      params.set("q", input.query.trim());
    }
    if (input.orderBy !== undefined && (input.orderBy !== "startTime" || input.singleEvents)) {
      params.set("orderBy", input.orderBy);
    }
    if (input.pageToken !== undefined && input.pageToken.length > 0) {
      params.set("pageToken", input.pageToken);
    }
    return this.#requestJson(`/calendar/v3/calendars/${encodeURIComponent(cal)}/events?${params}`);
  }

  async getEvent(rawCalendarId: string, rawEventId: string): Promise<JsonObject> {
    const cal = calendarId(rawCalendarId);
    const id = eventId(rawEventId);
    return this.#requestJson(
      `/calendar/v3/calendars/${encodeURIComponent(cal)}/events/${encodeURIComponent(id)}`,
    );
  }

  async createEvent(input: CreateEventInput): Promise<JsonObject> {
    const cal = calendarId(input.calendarId);
    const body: JsonObject = {
      summary: input.summary,
      start: eventTimeBound(input.start, input.allDay, input.timeZone, "start"),
      end: eventTimeBound(input.end, input.allDay, input.timeZone, "end"),
      ...(input.description === undefined ? {} : { description: input.description }),
      ...(input.location === undefined ? {} : { location: input.location }),
      ...(input.attendees === undefined
        ? {}
        : { attendees: input.attendees.map((email) => ({ email: email.trim() })) }),
    };
    const params = new URLSearchParams({ sendUpdates: input.sendUpdates });
    return this.#requestJson(
      `/calendar/v3/calendars/${encodeURIComponent(cal)}/events?${params}`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json; charset=utf-8" },
        body: JSON.stringify(body),
      },
    );
  }

  async updateEvent(input: UpdateEventInput): Promise<JsonObject> {
    const cal = calendarId(input.calendarId);
    const id = eventId(input.eventId);
    const body: JsonObject = {
      ...(input.summary === undefined ? {} : { summary: input.summary }),
      ...(input.start === undefined
        ? {}
        : { start: eventTimeBound(input.start, input.allDay, input.timeZone, "start") }),
      ...(input.end === undefined
        ? {}
        : { end: eventTimeBound(input.end, input.allDay, input.timeZone, "end") }),
      ...(input.description === undefined ? {} : { description: input.description }),
      ...(input.location === undefined ? {} : { location: input.location }),
      ...(input.attendees === undefined
        ? {}
        : { attendees: input.attendees.map((email) => ({ email: email.trim() })) }),
    };
    const params = new URLSearchParams({ sendUpdates: input.sendUpdates });
    return this.#requestJson(
      `/calendar/v3/calendars/${encodeURIComponent(cal)}/events/${encodeURIComponent(id)}?${params}`,
      {
        method: "PATCH",
        headers: { "Content-Type": "application/json; charset=utf-8" },
        body: JSON.stringify(body),
      },
    );
  }

  async deleteEvent(
    rawCalendarId: string,
    rawEventId: string,
    sendUpdates: SendUpdates,
  ): Promise<JsonObject> {
    const cal = calendarId(rawCalendarId);
    const id = eventId(rawEventId);
    const params = new URLSearchParams({ sendUpdates });
    const response = await this.#request(
      `/calendar/v3/calendars/${encodeURIComponent(cal)}/events/${encodeURIComponent(id)}?${params}`,
      { method: "DELETE" },
    );
    await response.body?.cancel();
    return { deleted: true, calendarId: cal, eventId: id };
  }

  async queryFreeBusy(
    timeMin: string,
    timeMax: string,
    calendarIds: readonly string[],
    timeZone?: string,
  ): Promise<JsonObject> {
    const body: JsonObject = {
      timeMin: rfc3339(timeMin, "timeMin"),
      timeMax: rfc3339(timeMax, "timeMax"),
      items: calendarIds.map((id) => ({ id: calendarId(id) })),
      ...(timeZone === undefined ? {} : { timeZone: timeZone.trim() }),
    };
    return this.#requestJson("/calendar/v3/freeBusy", {
      method: "POST",
      headers: { "Content-Type": "application/json; charset=utf-8" },
      body: JSON.stringify(body),
    });
  }

  async #requestJson(path: string, init: RequestInit = {}): Promise<JsonObject> {
    const response = await this.#request(path, init);
    const bytes = await readBounded(response, MAX_JSON_RESPONSE_BYTES);
    let value: unknown;
    try {
      value = JSON.parse(new TextDecoder().decode(bytes));
    } catch {
      throw new CalendarApiError(502, "Google Calendar returned invalid JSON.");
    }
    return requireJsonObject(value, "JSON response");
  }

  async #request(path: string, init: RequestInit = {}): Promise<Response> {
    const headers = new Headers(init.headers);
    headers.set("Accept", "application/json");
    headers.set("Authorization", `Bearer ${this.#token}`);
    const signal = AbortSignal.any([
      this.#signal,
      AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    ]);
    let response: Response;
    try {
      response = await this.#fetch(new URL(path, GOOGLE_API_ORIGIN), {
        ...init,
        headers,
        redirect: "manual",
        signal,
      });
    } catch (error) {
      if (signal.aborted) {
        throw new CalendarApiError(504, "Google Calendar request was cancelled or timed out.");
      }
      const message = error instanceof Error ? error.message.slice(0, 256) : "network failure";
      throw new CalendarApiError(503, `Google Calendar could not be reached: ${message}`);
    }
    if (response.status >= 300 && response.status < 400) {
      await response.body?.cancel();
      throw new CalendarApiError(502, "Google Calendar returned an unexpected redirect.");
    }
    if (!response.ok) {
      throw await calendarFailure(response);
    }
    return response;
  }
}

async function calendarFailure(response: Response): Promise<CalendarApiError> {
  let message = `Google Calendar returned HTTP ${response.status}.`;
  let reason: string | undefined;
  try {
    const bytes = await readBounded(response, 64 * 1024);
    const value: unknown = JSON.parse(new TextDecoder().decode(bytes));
    const object = requireJsonObject(value, "error response");
    const error = object.error;
    if (typeof error === "object" && error !== null && !Array.isArray(error)) {
      const candidate = Object.getOwnPropertyDescriptor(error, "message")?.value;
      if (typeof candidate === "string" && candidate.trim().length > 0) {
        message = candidate.trim().slice(0, 512);
      }
      const errors = Object.getOwnPropertyDescriptor(error, "errors")?.value;
      if (Array.isArray(errors)) {
        for (const item of errors) {
          if (typeof item !== "object" || item === null || Array.isArray(item)) continue;
          const candidateReason = Object.getOwnPropertyDescriptor(item, "reason")?.value;
          if (typeof candidateReason === "string" && candidateReason.length > 0) {
            reason = candidateReason.slice(0, 128);
            break;
          }
        }
      }
    }
  } catch {
    // Keep status-only error when response body is unreadable.
  }
  return new CalendarApiError(response.status, message, reason);
}

async function readBounded(response: Response, maxBytes: number): Promise<Uint8Array> {
  const contentLength = response.headers.get("content-length");
  if (contentLength !== null) {
    const parsed = Number(contentLength);
    if (Number.isFinite(parsed) && parsed > maxBytes) {
      await response.body?.cancel();
      throw new CalendarApiError(413, `Google Calendar response exceeds ${maxBytes} bytes.`);
    }
  }
  if (response.body === null) {
    return new Uint8Array();
  }
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  try {
    while (true) {
      const result = await reader.read();
      if (result.done) break;
      if (result.value !== undefined) {
        length += result.value.byteLength;
        if (length > maxBytes) {
          throw new CalendarApiError(413, `Google Calendar response exceeds ${maxBytes} bytes.`);
        }
        chunks.push(result.value);
      }
    }
  } finally {
    await reader.cancel().catch(() => undefined);
  }
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return bytes;
}

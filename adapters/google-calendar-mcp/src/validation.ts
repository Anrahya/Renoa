import { CalendarInputError } from "./errors.js";

const EVENT_ID = /^[A-Za-z0-9_.-]+$/;
const RFC3339 = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/;
const DATE_ONLY = /^\d{4}-\d{2}-\d{2}$/;

export function calendarId(value: string): string {
  const normalized = value.trim();
  if (
    normalized.length === 0 ||
    normalized.length > 256 ||
    /[\r\n\u0000/?#]/u.test(normalized)
  ) {
    throw new CalendarInputError("calendarId must be 'primary' or a valid Google Calendar ID.");
  }
  return normalized;
}

export function eventId(value: string): string {
  const normalized = value.trim();
  if (normalized.length === 0 || normalized.length > 1024 || !EVENT_ID.test(normalized)) {
    throw new CalendarInputError("eventId must be a valid Google Calendar event ID.");
  }
  return normalized;
}

export function rfc3339(value: string, field: string): string {
  const normalized = value.trim();
  if (!RFC3339.test(normalized)) {
    throw new CalendarInputError(
      `${field} must be an RFC3339 timestamp with offset (e.g. '2026-11-26T09:00:00+05:30').`,
    );
  }
  return normalized;
}

export function eventTimeBound(
  value: string,
  allDay: boolean,
  timeZone: string | undefined,
  field: string,
): { date: string } | { dateTime: string; timeZone?: string } {
  const normalized = value.trim();
  if (allDay) {
    if (!DATE_ONLY.test(normalized)) {
      throw new CalendarInputError(`${field} must be YYYY-MM-DD when allDay is true.`);
    }
    return { date: normalized };
  }
  return {
    dateTime: rfc3339(normalized, field),
    ...(timeZone === undefined ? {} : { timeZone: timeZone.trim() }),
  };
}

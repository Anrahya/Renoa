export class CalendarApiError extends Error {
  readonly status: number;
  readonly reason: string | undefined;

  constructor(status: number, message: string, reason?: string) {
    super(message);
    this.name = "CalendarApiError";
    this.status = status;
    this.reason = reason;
  }
}

export class CalendarInputError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "CalendarInputError";
  }
}

export function publicToolError(error: unknown, token: string): {
  error: string;
  status?: number;
  reason?: string;
} {
  if (error instanceof CalendarApiError) {
    return {
      error: redact(error.message, token),
      status: error.status,
      ...(error.reason === undefined ? {} : { reason: redact(error.reason, token) }),
    };
  }
  if (error instanceof CalendarInputError) {
    return { error: error.message };
  }
  return { error: "Google Calendar request failed unexpectedly." };
}

function redact(value: string, token: string): string {
  return value.replaceAll(token, "[REDACTED]").slice(0, 512);
}

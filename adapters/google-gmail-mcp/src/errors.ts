export class GmailApiError extends Error {
  readonly status: number;
  readonly reason: string | undefined;

  constructor(status: number, message: string, reason?: string) {
    super(message);
    this.name = "GmailApiError";
    this.status = status;
    this.reason = reason;
  }
}

export class GmailInputError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "GmailInputError";
  }
}

export function publicToolError(error: unknown, token: string): {
  error: string;
  status?: number;
  reason?: string;
} {
  if (error instanceof GmailApiError) {
    return {
      error: redact(error.message, token),
      status: error.status,
      ...(error.reason === undefined ? {} : { reason: redact(error.reason, token) }),
    };
  }
  if (error instanceof GmailInputError) {
    return { error: error.message };
  }
  return { error: "Google Gmail request failed unexpectedly." };
}

function redact(value: string, token: string): string {
  return value.replaceAll(token, "[REDACTED]").slice(0, 512);
}

export class DriveApiError extends Error {
  readonly status: number;
  readonly reason: string | undefined;

  constructor(status: number, message: string, reason?: string) {
    super(message);
    this.name = "DriveApiError";
    this.status = status;
    this.reason = reason;
  }
}

export class DriveInputError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "DriveInputError";
  }
}

export function publicToolError(error: unknown, token: string): {
  error: string;
  status?: number;
  reason?: string;
} {
  if (error instanceof DriveApiError) {
    return {
      error: redact(error.message, token),
      status: error.status,
      ...(error.reason === undefined ? {} : { reason: redact(error.reason, token) }),
    };
  }
  if (error instanceof DriveInputError) {
    return { error: error.message };
  }
  return { error: "Google Drive request failed unexpectedly." };
}

function redact(value: string, token: string): string {
  return value.replaceAll(token, "[REDACTED]").slice(0, 512);
}

/**
 * Reading what a provider's error carries: an HTTP status, the provider's own
 * message and its body, from the shapes the SDKs Pi uses throw them in.
 */

export function parseFormattedHttpError(message: string): {
  status?: number;
  message: string;
  body?: Record<string, unknown>;
} {
  const match = /^(\d{3}):\s*([\s\S]*)$/.exec(message);
  if (match === null || match[1] === undefined || match[2] === undefined) {
    return googleError(message) ?? { message };
  }
  const status = Number(match[1]);
  const rest = match[2];
  try {
    const parsed = JSON.parse(rest) as unknown;
    if (typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)) {
      return { status, message: rest, body: parsed as Record<string, unknown> };
    }
  } catch {
    // The remainder is not JSON; keep it as the human-readable message.
  }
  return { status, message: rest };
}

/**
 * Google's SDK throws the response body itself as the message:
 * `{"error":{"code":429,"message":...,"status":...}}`, and Pi's Vertex path
 * may wrap that body once more as the inner `message`. The HTTP status is the
 * error's numeric `code`; the message is the innermost one.
 */
function googleError(message: string): { status: number; message: string; body: Record<string, unknown> } | undefined {
  const body = jsonObject(message);
  const error = nestedRecord(body?.error);
  const status = numeric(error?.code);
  if (body === undefined || error === undefined || status === undefined || status < 100 || status > 599) {
    return undefined;
  }
  const said = stringField(error.message) ?? message;
  const inner = googleError(said);
  return { status, message: inner?.message ?? said, body };
}

function jsonObject(text: string): Record<string, unknown> | undefined {
  try {
    const parsed = JSON.parse(text) as unknown;
    return typeof parsed === "object" && parsed !== null && !Array.isArray(parsed)
      ? (parsed as Record<string, unknown>)
      : undefined;
  } catch {
    return undefined;
  }
}

export function numeric(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

export function stringField(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

export function nestedRecord(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

export function headerMap(value: unknown): Record<string, string> {
  if (value instanceof Headers) {
    return Object.fromEntries(value.entries());
  }
  if (typeof value !== "object" || value === null) {
    return {};
  }
  const result: Record<string, string> = {};
  for (const [key, nested] of Object.entries(value)) {
    if (typeof nested === "string") {
      result[key.toLowerCase()] = nested;
    }
  }
  return result;
}

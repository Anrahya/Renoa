import { providerDisplayName, type FailureCategory, type InferenceOutcome } from "./contract.js";
import { boundProviderMessage, redactText } from "./redact.js";

export { redactHeaders, redactSecrets, redactText } from "./redact.js";

export interface ClassifyOptions {
  readonly outputExposed: boolean;
  readonly cancelled: boolean;
  readonly dispatched?: boolean;
}

export interface FailureFacts {
  readonly category: FailureCategory;
  readonly inferenceOutcome: InferenceOutcome;
  readonly httpStatus?: number;
  readonly providerCode?: string;
  readonly requestId?: string;
  readonly retryAfter?: string;
  readonly causeCode?: string;
  readonly causeMessage?: string;
  readonly providerMessage?: string;
  readonly retryable: boolean;
  readonly expiredOAuth: boolean;
  readonly rawMessage: string;
}

export class ProviderFailure extends Error {
  readonly category: FailureCategory;
  readonly inferenceOutcome: InferenceOutcome;
  readonly httpStatus?: number;
  readonly providerCode?: string;
  readonly requestId?: string;
  readonly retryAfter?: string;
  readonly causeCode?: string;
  readonly causeMessage?: string;
  readonly providerMessage?: string;
  readonly retryable: boolean;
  readonly expiredOAuth: boolean;
  readonly attemptCount: number;
  readonly provider: string;
  readonly model: string;

  constructor(
    facts: FailureFacts,
    options: {
      readonly provider: string;
      readonly model: string;
      readonly attemptCount: number;
    },
  ) {
    super(conciseMessage(options.provider, options.attemptCount, facts));
    this.name = "ProviderFailure";
    this.category = facts.category;
    this.inferenceOutcome = facts.inferenceOutcome;
    this.retryable = facts.retryable;
    this.expiredOAuth = facts.expiredOAuth;
    this.attemptCount = options.attemptCount;
    this.provider = options.provider;
    this.model = options.model;
    if (facts.httpStatus !== undefined) {
      this.httpStatus = facts.httpStatus;
    }
    if (facts.providerCode !== undefined) {
      this.providerCode = facts.providerCode;
    }
    if (facts.requestId !== undefined) {
      this.requestId = facts.requestId;
    }
    if (facts.retryAfter !== undefined) {
      this.retryAfter = facts.retryAfter;
    }
    if (facts.causeCode !== undefined) {
      this.causeCode = facts.causeCode;
    }
    if (facts.causeMessage !== undefined) {
      this.causeMessage = facts.causeMessage;
    }
    if (facts.providerMessage !== undefined && facts.providerMessage.length > 0) {
      this.providerMessage = facts.providerMessage;
    }
  }
}

export function classifyError(error: unknown, options: ClassifyOptions): FailureFacts {
  if (options.cancelled) {
    return {
      category: "cancelled",
      inferenceOutcome: options.outputExposed || options.dispatched === true ? "unknown" : "known_not_started",
      retryable: false,
      expiredOAuth: false,
      rawMessage: "request was cancelled",
    };
  }

  const extracted = extractError(error);
  const classified = classifyCategory(extracted, categoryHint(error));
  const category =
    options.outputExposed &&
    (classified === "network" || classified === "timeout" || classified === "provider_unavailable")
      ? "stream_interrupted"
      : classified;
  const expiredOAuth = isExpiredOAuth(extracted);
  const retryable =
    !options.outputExposed &&
    category !== "stream_interrupted" &&
    !expiredOAuth &&
    isRetryable(category, extracted.status);
  const inferenceOutcome = inferenceFor(category, extracted.status, options);
  const providerMessage = boundProviderMessage(extracted.message);
  return {
    category,
    inferenceOutcome,
    retryable,
    expiredOAuth,
    rawMessage: redactText(extracted.message),
    ...(providerMessage.length > 0 ? { providerMessage } : {}),
    ...(extracted.status !== undefined ? { httpStatus: extracted.status } : {}),
    ...(extracted.providerCode !== undefined ? { providerCode: extracted.providerCode } : {}),
    ...(extracted.requestId !== undefined ? { requestId: extracted.requestId } : {}),
    ...(extracted.retryAfter !== undefined ? { retryAfter: extracted.retryAfter } : {}),
    ...(extracted.causeCode !== undefined ? { causeCode: extracted.causeCode } : {}),
    ...(extracted.causeMessage !== undefined ? { causeMessage: boundProviderMessage(extracted.causeMessage) } : {}),
  };
}

function conciseMessage(provider: string, attempts: number, facts: FailureFacts): string {
  const name = providerDisplayName(provider);
  const attemptWord = attempts === 1 ? "attempt" : "attempts";
  return `${name} request failed after ${attempts} ${attemptWord}: ${summarize(facts)}`;
}

function summarize(facts: FailureFacts): string {
  switch (facts.category) {
    case "cancelled":
      return "cancelled.";
    case "authentication":
      return facts.expiredOAuth
        ? "authentication failed after token refresh."
        : "authentication failed.";
    case "rate_limited": {
      const status = facts.httpStatus ?? 429;
      const id = facts.requestId === undefined ? "" : ` (request ${facts.requestId})`;
      return `rate limited (${status})${id}.`;
    }
    case "invalid_request":
      return `invalid request${facts.httpStatus === undefined ? "" : ` (${facts.httpStatus})`}.`;
    case "context_window_exceeded":
      return "context window exceeded.";
    case "timeout":
      return "timed out.";
    case "provider_unavailable":
      return `provider unavailable${facts.httpStatus === undefined ? "" : ` (${facts.httpStatus})`}.`;
    case "protocol":
      return "malformed provider response.";
    case "stream_interrupted":
      return "stream interrupted after output; inference outcome is unknown.";
    case "network": {
      const code = facts.causeCode ?? errnoFromMessage(facts.rawMessage);
      if (code !== undefined && facts.httpStatus === undefined) {
        return facts.inferenceOutcome === "unknown"
          ? `connection reset after the request may have been transmitted (${code}).`
          : `connection reset before an HTTP response (${code}).`;
      }
      if (code !== undefined) {
        return `network error (${code}).`;
      }
      return facts.httpStatus === undefined
        ? facts.inferenceOutcome === "unknown"
          ? "connection failed after the request may have been transmitted."
          : "connection failed before an HTTP response."
        : `network error (${facts.httpStatus}).`;
    }
    case "unknown":
      return redactText(facts.rawMessage || "unknown provider error.");
  }
}

interface ExtractedError {
  readonly message: string;
  readonly status?: number;
  readonly providerCode?: string;
  readonly requestId?: string;
  readonly retryAfter?: string;
  readonly causeCode?: string;
  readonly causeMessage?: string;
}

function extractError(error: unknown): ExtractedError {
  if (typeof error === "string") {
    return { message: redactText(error) };
  }
  if (typeof error !== "object" || error === null) {
    return { message: redactText(String(error)) };
  }
  const record = error as Record<string, unknown>;
  const cause = nestedCause(record);
  const headers = headerMap(record.headers);
  const parsed = parseFormattedHttpError(errorMessage(error));
  const status = numeric(record.status) ?? numeric(record.statusCode) ?? parsed.status;
  const message = redactText(parsed.message);
  const body = parsed.body;
  const providerCode =
    stringField(record.code) ??
    stringField(nestedRecord(record.error)?.code) ??
    stringField(nestedRecord(record.error)?.type) ??
    stringField(body?.code) ??
    stringField(body?.type) ??
    stringField(nestedRecord(body?.error)?.code) ??
    stringField(nestedRecord(body?.error)?.type);
  const requestId =
    stringField(record.requestID) ??
    stringField(record.request_id) ??
    stringField(record.requestId) ??
    headers["x-request-id"] ??
    headers["request-id"];
  const retryAfter = headers["retry-after"];
  return {
    message,
    ...(status !== undefined ? { status } : {}),
    ...(providerCode !== undefined ? { providerCode: redactText(providerCode) } : {}),
    ...(requestId !== undefined ? { requestId: redactText(requestId) } : {}),
    ...(retryAfter !== undefined ? { retryAfter: redactText(retryAfter) } : {}),
    ...(cause?.code !== undefined ? { causeCode: cause.code } : {}),
    ...(cause?.message !== undefined ? { causeMessage: redactText(cause.message) } : {}),
  };
}

function nestedCause(record: Record<string, unknown>): { code?: string; message?: string } | undefined {
  const cause = record.cause;
  if (typeof cause !== "object" || cause === null) {
    const code = errnoFromMessage(errorMessage(record));
    return code === undefined ? undefined : { code, message: errorMessage(record) };
  }
  const nested = cause as Record<string, unknown>;
  const deeper = typeof nested.cause === "object" && nested.cause !== null ? (nested.cause as Record<string, unknown>) : nested;
  const code = stringField(deeper.code) ?? stringField(nested.code) ?? errnoFromMessage(errorMessage(cause));
  const message = errorMessage(cause);
  if (code === undefined && message.length === 0) {
    return undefined;
  }
  return {
    ...(code !== undefined ? { code } : {}),
    ...(message.length > 0 ? { message } : {}),
  };
}

function classifyCategory(extracted: ExtractedError, hint: FailureCategory | undefined): FailureCategory {
  if (hint === "invalid_request" || hint === "context_window_exceeded") {
    return hint;
  }
  if (isContextOverflowMessage(extracted.message)) {
    return "context_window_exceeded";
  }
  if (/credentials are not configured|no api key|provider is not configured|oauth refresh|expired access token|invalid_grant/i.test(extracted.message)) {
    return "authentication";
  }
  if (extracted.status === 401 || extracted.status === 403) {
    return "authentication";
  }
  if (extracted.status === 429) {
    return "rate_limited";
  }
  if (extracted.status === 408) {
    return "timeout";
  }
  if (
    extracted.status === 400 ||
    extracted.status === 404 ||
    extracted.status === 409 ||
    extracted.status === 422
  ) {
    return "invalid_request";
  }
  if (
    extracted.status === 500 ||
    extracted.status === 502 ||
    extracted.status === 503 ||
    extracted.status === 504 ||
    extracted.status === 529
  ) {
    return "provider_unavailable";
  }
  if (isTimeout(extracted)) {
    return "timeout";
  }
  if (isProtocol(extracted)) {
    return "protocol";
  }
  if (isNetwork(extracted)) {
    return "network";
  }
  if (/authentication|unauthorized|invalid api key|invalid token/i.test(extracted.message)) {
    return "authentication";
  }
  if (/rate.?limit|too many requests/i.test(extracted.message)) {
    return "rate_limited";
  }
  return "unknown";
}

function categoryHint(error: unknown): FailureCategory | undefined {
  if (typeof error !== "object" || error === null || !("categoryHint" in error)) {
    return undefined;
  }
  const hint = (error as { categoryHint?: unknown }).categoryHint;
  return hint === "invalid_request" || hint === "context_window_exceeded" ? hint : undefined;
}

function isRetryable(category: FailureCategory, status: number | undefined): boolean {
  if (status !== undefined && status >= 400 && status < 500 && status !== 408 && status !== 429) {
    return false;
  }
  return (
    category === "network" ||
    category === "timeout" ||
    category === "rate_limited" ||
    category === "provider_unavailable" ||
    status === 408 ||
    status === 429 ||
    (status !== undefined && status >= 500 && status !== 501)
  );
}

function inferenceFor(
  category: FailureCategory,
  status: number | undefined,
  options: ClassifyOptions,
): InferenceOutcome {
  if (options.outputExposed) {
    return "unknown";
  }
  if (options.dispatched === true) {
    return explicitClientRejection(status) ? "known_not_started" : "unknown";
  }
  if (providerRejectedBeforeInference(category, status)) {
    return "known_not_started";
  }
  if (category === "network" || category === "authentication" || category === "invalid_request") {
    return "known_not_started";
  }
  return "unknown";
}

function explicitClientRejection(status: number | undefined): boolean {
  return status !== undefined && status >= 400 && status < 500;
}

function providerRejectedBeforeInference(category: FailureCategory, status: number | undefined): boolean {
  if (
    category === "authentication" ||
    category === "invalid_request" ||
    category === "context_window_exceeded" ||
    category === "rate_limited"
  ) {
    return true;
  }
  return explicitClientRejection(status);
}

function isExpiredOAuth(extracted: ExtractedError): boolean {
  if (extracted.status !== 401) {
    return false;
  }
  const haystack = `${extracted.providerCode ?? ""} ${extracted.message}`.toLowerCase();
  return (
    haystack.includes("invalid_token") ||
    haystack.includes("token expired") ||
    haystack.includes("expired token") ||
    haystack.includes("access token expired")
  );
}

function isNetwork(extracted: ExtractedError): boolean {
  const code = extracted.causeCode ?? "";
  const message = extracted.message;
  return (
    /ECONNRESET|ENOTFOUND|EAI_AGAIN|ECONNREFUSED|ETIMEDOUT|EPIPE|UND_ERR|EHOSTUNREACH/i.test(code) ||
    /ECONNRESET|ENOTFOUND|EAI_AGAIN|ECONNREFUSED|ETIMEDOUT|fetch failed|APIConnectionError|other side closed|socket hang up|reset before|connection (?:reset|refused|error)/i.test(
      message,
    )
  );
}

function isTimeout(extracted: ExtractedError): boolean {
  return (
    extracted.causeCode === "ETIMEDOUT" ||
    extracted.causeCode === "UND_ERR_CONNECT_TIMEOUT" ||
    /timed? ?out|TimeoutError|deadline/i.test(extracted.message)
  );
}

function isProtocol(extracted: ExtractedError): boolean {
  return /malformed|unexpected end|invalid json|unexpected token|SSE|not valid JSON|unterminated|Could not parse|From chunk|JSON at position|Expected property name|in JSON at|SyntaxError/i.test(
    extracted.message,
  );
}

function isContextOverflowMessage(message: string): boolean {
  return /context window|prompt is too long|maximum prompt length|request_too_large|context[_ ]length[_ ]exceeded|token limit exceeded/i.test(
    message,
  );
}

function errnoFromMessage(message: string): string | undefined {
  const match = /(?:code:\s*)?(ECONNRESET|ENOTFOUND|EAI_AGAIN|ECONNREFUSED|ETIMEDOUT|EPIPE|UND_ERR_[A-Z_]+)/i.exec(
    message,
  );
  return match?.[1]?.toUpperCase();
}

function parseFormattedHttpError(message: string): {
  status?: number;
  message: string;
  body?: Record<string, unknown>;
} {
  const match = /^(\d{3}):\s*([\s\S]*)$/.exec(message);
  if (match === null || match[1] === undefined || match[2] === undefined) {
    return { message };
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

function errorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "string") {
    return error;
  }
  try {
    return JSON.stringify(error) ?? String(error);
  } catch {
    return String(error);
  }
}

function numeric(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function stringField(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function nestedRecord(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function headerMap(value: unknown): Record<string, string> {
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

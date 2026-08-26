import {
  ProtocolError,
  ProtocolErrorCode,
  SdkError,
  SdkErrorCode,
  SdkHttpError,
} from "@modelcontextprotocol/client";
import type { AdapterFailureKind, WireFailure } from "./contract.js";
import { MAX_DIAGNOSTIC_BYTES, MAX_FAILURE_MESSAGE_BYTES } from "./limits.js";

export interface ExchangeEvidence {
  readonly dispatchStarted: boolean;
  readonly responseStarted: boolean;
}

export class AdapterProblem extends Error {
  readonly kind: AdapterFailureKind;
  readonly partialChangesPossible: boolean;
  readonly code?: string;
  readonly httpStatus?: number;

  constructor(
    kind: AdapterFailureKind,
    message: string,
    options: {
      readonly partialChangesPossible?: boolean;
      readonly code?: string;
      readonly httpStatus?: number;
      readonly cause?: unknown;
    } = {},
  ) {
    super(
      message,
      options.cause === undefined ? undefined : { cause: options.cause },
    );
    this.name = "AdapterProblem";
    this.kind = kind;
    this.partialChangesPossible = options.partialChangesPossible ?? false;
    if (options.code !== undefined) {
      this.code = options.code;
    }
    if (options.httpStatus !== undefined) {
      this.httpStatus = options.httpStatus;
    }
  }
}

export function toWireFailure(
  error: unknown,
  evidence: ExchangeEvidence = {
    dispatchStarted: false,
    responseStarted: false,
  },
  cancellationRequested = false,
): WireFailure {
  const problem = findAdapterProblem(error);
  const code = errorCode(error, problem);
  const httpStatus = httpStatusOf(error, problem);

  if (problem !== undefined) {
    return failure(
      problem.kind,
      "definite",
      problem.message,
      problem.partialChangesPossible,
      error,
      code,
      httpStatus,
    );
  }

  if (
    evidence.dispatchStarted &&
    evidence.responseStarted &&
    error instanceof ProtocolError
  ) {
    return failure(
      "protocol",
      "definite",
      `MCP server rejected the tool call: ${error.message}`,
      true,
      error,
      code,
      httpStatus,
    );
  }

  if (cancellationRequested || isCancellation(error)) {
    if (evidence.dispatchStarted) {
      return failure(
        "cancelled",
        "unknown",
        "MCP tool cancellation left the remote outcome unknown.",
        true,
        error,
        code,
        httpStatus,
      );
    }
    return failure(
      "cancelled",
      "definite",
      "MCP operation was cancelled before the tool call was dispatched.",
      false,
      error,
      code,
      httpStatus,
    );
  }

  if (isProvablyPreConnection(error)) {
    return failure(
      "unavailable",
      "definite",
      "The MCP endpoint could not receive the tool call.",
      false,
      error,
      code,
      httpStatus,
    );
  }

  if (evidence.dispatchStarted) {
    return failure(
      error instanceof SdkError && error.code === SdkErrorCode.RequestTimeout
        ? "timeout"
        : "transport",
      "unknown",
      "The MCP response was lost after dispatch; the tool outcome is unknown.",
      true,
      error,
      code,
      httpStatus,
    );
  }

  if (error instanceof SdkError) {
    switch (error.code) {
      case SdkErrorCode.EraNegotiationFailed:
      case SdkErrorCode.CapabilityNotSupported:
      case SdkErrorCode.MethodNotSupportedByProtocolVersion:
        return failure(
          "incompatible_protocol",
          "definite",
          "The endpoint does not support Renoa's pinned MCP protocol and tools capability.",
          false,
          error,
          code,
          httpStatus,
        );
      case SdkErrorCode.RequestTimeout:
        return failure(
          "timeout",
          "definite",
          "The MCP operation timed out before any tool call was dispatched.",
          false,
          error,
          code,
          httpStatus,
        );
      case SdkErrorCode.InvalidResult:
      case SdkErrorCode.UnsupportedResultType:
        return failure(
          "protocol",
          "definite",
          "The MCP endpoint returned an unsupported or malformed result.",
          false,
          error,
          code,
          httpStatus,
        );
      default:
        break;
    }
  }

  if (error instanceof SdkHttpError) {
    const kind: AdapterFailureKind =
      error.status >= 500 ? "unavailable" : "protocol";
    return failure(
      kind,
      "definite",
      `The MCP endpoint returned HTTP ${error.status} before any tool call was dispatched.`,
      false,
      error,
      code,
      error.status,
    );
  }

  if (error instanceof ProtocolError) {
    if (error.code === ProtocolErrorCode.UnsupportedProtocolVersion) {
      return failure(
        "incompatible_protocol",
        "definite",
        "The endpoint does not support Renoa's pinned MCP protocol version.",
        false,
        error,
        code,
        httpStatus,
      );
    }
    return failure(
      "protocol",
      "definite",
      `The MCP endpoint rejected the operation: ${error.message}`,
      false,
      error,
      code,
      httpStatus,
    );
  }

  return failure(
    "internal",
    "definite",
    "The MCP adapter failed before any tool call was dispatched.",
    false,
    error,
    code,
    httpStatus,
  );
}

export function safeDiagnostic(error: unknown): string {
  const seen = new Set<object>();
  const parts: string[] = [];
  let current: unknown = error;
  for (let depth = 0; depth < 6 && current !== undefined; depth += 1) {
    if (typeof current === "object" && current !== null) {
      if (seen.has(current)) {
        break;
      }
      seen.add(current);
    }
    parts.push(errorMessage(current));
    current = causeOf(current);
  }
  return boundUtf8(
    redact(parts.filter((part) => part.length > 0).join(": ")),
    MAX_DIAGNOSTIC_BYTES,
  );
}

function failure(
  kind: AdapterFailureKind,
  certainty: "definite" | "unknown",
  message: string,
  partialChangesPossible: boolean,
  error: unknown,
  code: string | undefined,
  httpStatus: number | undefined,
): WireFailure {
  return {
    kind,
    certainty,
    message: boundUtf8(redact(message), MAX_FAILURE_MESSAGE_BYTES),
    partial_changes_possible: partialChangesPossible,
    diagnostic: {
      ...(code === undefined ? {} : { code: boundUtf8(redact(code), 128) }),
      ...(httpStatus === undefined ? {} : { http_status: httpStatus }),
      detail: safeDiagnostic(error),
    },
  };
}

function findAdapterProblem(error: unknown): AdapterProblem | undefined {
  let current: unknown = error;
  const seen = new Set<object>();
  for (let depth = 0; depth < 6 && current !== undefined; depth += 1) {
    if (current instanceof AdapterProblem) {
      return current;
    }
    if (typeof current !== "object" || current === null || seen.has(current)) {
      return undefined;
    }
    seen.add(current);
    current = causeOf(current);
  }
  return undefined;
}

function errorCode(
  error: unknown,
  problem: AdapterProblem | undefined,
): string | undefined {
  if (problem?.code !== undefined) {
    return problem.code;
  }
  if (error instanceof ProtocolError) {
    return `json_rpc_${error.code}`;
  }
  if (error instanceof SdkError) {
    return error.code;
  }
  return nestedStringField(error, "code");
}

function httpStatusOf(
  error: unknown,
  problem: AdapterProblem | undefined,
): number | undefined {
  if (problem?.httpStatus !== undefined) {
    return problem.httpStatus;
  }
  return error instanceof SdkHttpError ? error.status : undefined;
}

function isCancellation(error: unknown): boolean {
  return (
    error instanceof Error &&
    (error.name === "AbortError" || /\bcancell?ed\b/i.test(error.message))
  );
}

function isProvablyPreConnection(error: unknown): boolean {
  const code = nestedStringField(error, "code");
  return (
    code === "ENOTFOUND" ||
    code === "ECONNREFUSED" ||
    code === "ERR_TLS_CERT_ALTNAME_INVALID" ||
    code === "DEPTH_ZERO_SELF_SIGNED_CERT" ||
    code === "UNABLE_TO_VERIFY_LEAF_SIGNATURE" ||
    code === "CERT_HAS_EXPIRED"
  );
}

function nestedStringField(error: unknown, field: string): string | undefined {
  let current: unknown = error;
  const seen = new Set<object>();
  for (let depth = 0; depth < 6; depth += 1) {
    if (typeof current !== "object" || current === null || seen.has(current)) {
      return undefined;
    }
    seen.add(current);
    const value = (current as Record<string, unknown>)[field];
    if (typeof value === "string" && value.length > 0) {
      return value;
    }
    current = causeOf(current);
  }
  return undefined;
}

function causeOf(error: unknown): unknown {
  return typeof error === "object" && error !== null
    ? (error as { readonly cause?: unknown }).cause
    : undefined;
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) {
    return `${error.name}: ${error.message}`;
  }
  if (typeof error === "string") {
    return error;
  }
  if (error === undefined) {
    return "";
  }
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

function redact(text: string): string {
  return text
    .replace(
      /-----BEGIN [^-\r\n]*PRIVATE KEY-----[\s\S]*?(?:-----END [^-\r\n]*PRIVATE KEY-----|$)/gi,
      "[REDACTED PRIVATE KEY]",
    )
    .replace(
      /(authorization\s*[:=]\s*)(?:bearer|apikey)?\s*[^\s,;]+/gi,
      "$1[REDACTED]",
    )
    .replace(
      /((?:access[_-]?token|refresh[_-]?token|api[_-]?key|client[_-]?secret|password)\s*["']?\s*[:=]\s*["']?)[^"'\s,}]+/gi,
      "$1[REDACTED]",
    );
}

export function boundUtf8(text: string, maxBytes: number): string {
  const encoded = Buffer.from(text, "utf8");
  if (encoded.byteLength <= maxBytes) {
    return text;
  }
  const suffix = "…";
  const budget = Math.max(0, maxBytes - Buffer.byteLength(suffix));
  return `${encoded
    .subarray(0, budget)
    .toString("utf8")
    .replace(/\uFFFD$/u, "")}${suffix}`;
}

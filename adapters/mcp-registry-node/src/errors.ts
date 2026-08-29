import type { FailureKind, WireFailure } from "./contract.js";
import {
  MAX_DIAGNOSTIC_BYTES,
  MAX_FAILURE_MESSAGE_BYTES,
} from "./limits.js";

export class RegistryProblem extends Error {
  readonly kind: FailureKind;
  readonly code?: string;
  readonly httpStatus?: number;

  constructor(
    kind: FailureKind,
    message: string,
    options: {
      readonly code?: string;
      readonly httpStatus?: number;
      readonly cause?: unknown;
    } = {},
  ) {
    super(
      message,
      options.cause === undefined ? undefined : { cause: options.cause },
    );
    this.name = "RegistryProblem";
    this.kind = kind;
    if (options.code !== undefined) {
      this.code = options.code;
    }
    if (options.httpStatus !== undefined) {
      this.httpStatus = options.httpStatus;
    }
  }
}

export function toWireFailure(error: unknown): WireFailure {
  const problem = error instanceof RegistryProblem ? error : undefined;
  return {
    kind: problem?.kind ?? "internal",
    message: bounded(
      problem?.message ??
        "The official MCP Registry adapter failed before returning a result.",
      MAX_FAILURE_MESSAGE_BYTES,
    ),
    diagnostic: {
      ...(problem?.code === undefined ? {} : { code: problem.code }),
      ...(problem?.httpStatus === undefined
        ? {}
        : { http_status: problem.httpStatus }),
      detail: safeDiagnostic(error),
    },
  };
}

export function safeDiagnostic(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error);
  return bounded(text.replace(/[\u0000-\u001F\u007F]/gu, " "), MAX_DIAGNOSTIC_BYTES);
}

function bounded(value: string, bytes: number): string {
  const encoded = Buffer.from(value, "utf8");
  if (encoded.byteLength <= bytes) {
    return value;
  }
  return encoded.subarray(0, bytes).toString("utf8");
}

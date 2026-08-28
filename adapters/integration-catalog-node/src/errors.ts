export type ProblemKind =
  | "invalid_request"
  | "not_found"
  | "conflict"
  | "unavailable"
  | "protocol"
  | "resource_limit"
  | "internal";

export class CatalogProblem extends Error {
  readonly kind: ProblemKind;
  readonly code?: string;
  readonly httpStatus?: number;

  constructor(
    kind: ProblemKind,
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
    this.name = "CatalogProblem";
    this.kind = kind;
    if (options.code !== undefined) {
      this.code = options.code;
    }
    if (options.httpStatus !== undefined) {
      this.httpStatus = options.httpStatus;
    }
  }
}

export function failure(error: unknown): {
  readonly kind: ProblemKind;
  readonly message: string;
  readonly diagnostic: { readonly code?: string; readonly http_status?: number; readonly detail: string };
} {
  const problem = error instanceof CatalogProblem ? error : undefined;
  const detail = safeDiagnostic(error);
  return {
    kind: problem?.kind ?? "internal",
    message:
      problem?.message ??
      "The integration discovery adapter failed before returning a result.",
    diagnostic: {
      ...(problem?.code === undefined ? {} : { code: problem.code }),
      ...(problem?.httpStatus === undefined
        ? {}
        : { http_status: problem.httpStatus }),
      detail,
    },
  };
}

export function safeDiagnostic(error: unknown): string {
  const text = error instanceof Error ? error.message : String(error);
  return text.replace(/[\u0000-\u001F\u007F]/gu, " ").slice(0, 4_096);
}

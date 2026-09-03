import { InsufficientScopeError } from "@modelcontextprotocol/client";
import type { WireFailure } from "./contract.js";
import { challengedScope } from "./oauth-scope.js";

export function insufficientScopeFailure(error: unknown): WireFailure | undefined {
  const scopeError = findInsufficientScope(error);
  if (scopeError === undefined) return undefined;
  const requiredScope = challengedScope(scopeError.requiredScope);
  return {
    kind: "protocol",
    certainty: "definite",
    message: "The MCP server requires additional OAuth authorization for this operation.",
    partial_changes_possible: false,
    diagnostic: {
      code: "oauth_insufficient_scope",
      http_status: 403,
      ...(requiredScope === undefined ? {} : { required_scope: requiredScope }),
      detail:
        scopeError.requiredScope !== undefined && requiredScope === undefined
          ? "The server returned a malformed or oversized OAuth scope challenge."
          : "The protected resource returned HTTP 403 insufficient_scope.",
    },
  };
}

function findInsufficientScope(error: unknown): InsufficientScopeError | undefined {
  let current: unknown = error;
  const seen = new Set<object>();
  for (let depth = 0; depth < 6 && current !== undefined; depth += 1) {
    if (current instanceof InsufficientScopeError) return current;
    if (typeof current !== "object" || current === null || seen.has(current)) {
      return undefined;
    }
    seen.add(current);
    current = (current as { readonly cause?: unknown }).cause;
  }
  return undefined;
}

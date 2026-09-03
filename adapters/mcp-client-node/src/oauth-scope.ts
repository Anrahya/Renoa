import {
  computeScopeUnion,
  isStrictScopeSuperset,
} from "@modelcontextprotocol/client";
import { MAX_OAUTH_SCOPE_BYTES } from "./limits.js";

const OAUTH_SCOPE = /^[\x21\x23-\x5B\x5D-\x7E]+(?: [\x21\x23-\x5B\x5D-\x7E]+)*$/u;

export function isValidOAuthScope(value: string): boolean {
  return (
    Buffer.byteLength(value, "utf8") <= MAX_OAUTH_SCOPE_BYTES &&
    OAUTH_SCOPE.test(value)
  );
}

export function challengedScope(value: string | undefined): string | undefined {
  if (value === undefined) return undefined;
  return isValidOAuthScope(value) ? value : undefined;
}

export function scopeUpgrade(
  granted: string | undefined,
  requested: string | undefined,
): { readonly scope: string | undefined; readonly widensGrant: boolean } {
  const scope = computeScopeUnion(granted, requested);
  return {
    scope,
    widensGrant:
      requested !== undefined && isStrictScopeSuperset(scope, granted),
  };
}

import type { CatalogRequest } from "./contract.js";
import { CatalogProblem } from "./errors.js";
import { MAX_QUERY_BYTES, WIRE_VERSION } from "./limits.js";

const REFERENCE = /^integrations\.sh\/([a-z0-9.-]+)\/([a-z0-9-]+)\/([a-f0-9]{64})$/u;

export function parseRequest(value: unknown): CatalogRequest {
  const request = object(value, "request");
  if (request.wire_version !== WIRE_VERSION) {
    throw invalid(`request.wire_version must be ${WIRE_VERSION}`);
  }
  if (request.action === "search") {
    exactKeys(request, ["wire_version", "action", "query"], "request");
    const query = string(request.query, "request.query").trim();
    if (query.length === 0 || Buffer.byteLength(query, "utf8") > MAX_QUERY_BYTES) {
      throw invalid(
        `request.query must contain 1 to ${MAX_QUERY_BYTES} UTF-8 bytes`,
      );
    }
    return { wire_version: WIRE_VERSION, action: "search", query };
  }
  if (request.action === "resolve") {
    exactKeys(request, ["wire_version", "action", "candidate"], "request");
    const candidate = string(request.candidate, "request.candidate");
    parseReference(candidate);
    return { wire_version: WIRE_VERSION, action: "resolve", candidate };
  }
  throw invalid("request.action must be 'search' or 'resolve'");
}

export function parseReference(reference: string): {
  readonly domain: string;
  readonly slug: string;
  readonly digest: string;
} {
  const match = REFERENCE.exec(reference);
  if (match === null) {
    throw invalid("candidate reference is malformed; search again for a current candidate");
  }
  const [, domain, slug, digest] = match;
  if (domain === undefined || slug === undefined || digest === undefined) {
    throw invalid("candidate reference could not be decoded");
  }
  return { domain, slug, digest };
}

function object(value: unknown, path: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw invalid(`${path} must be a JSON object`);
  }
  return value as Record<string, unknown>;
}

function string(value: unknown, path: string): string {
  if (typeof value !== "string") {
    throw invalid(`${path} must be a string`);
  }
  return value;
}

function exactKeys(
  value: Record<string, unknown>,
  keys: readonly string[],
  path: string,
): void {
  const expected = new Set(keys);
  for (const key of Object.keys(value)) {
    if (!expected.has(key)) {
      throw invalid(`${path} contains unknown field '${key}'`);
    }
  }
  for (const key of keys) {
    if (!(key in value)) {
      throw invalid(`${path} is missing required field '${key}'`);
    }
  }
}

function invalid(message: string): CatalogProblem {
  return new CatalogProblem("invalid_request", message, {
    code: "invalid_wire_request",
  });
}

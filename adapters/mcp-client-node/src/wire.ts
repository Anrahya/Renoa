import type {
  AdapterRequest,
  FrozenMcpTool,
  JsonObject,
  JsonValue,
  WireAuthorization,
  WireHeaders,
} from "./contract.js";
import { AdapterProblem } from "./errors.js";
import {
  MAX_AUTH_TOKEN_BYTES,
  MAX_REQUEST_HEADER_BYTES,
  MAX_REQUEST_HEADERS,
  WIRE_VERSION,
} from "./limits.js";

const HEADER_TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
const CLIENT_OWNED_HEADERS = new Set([
  "accept",
  "authorization",
  "connection",
  "content-length",
  "content-type",
  "cookie",
  "host",
  "mcp-method",
  "mcp-protocol-version",
  "mcp-session-id",
  "proxy-authorization",
  "set-cookie",
  "transfer-encoding",
  "x-api-key",
]);

export function parseAdapterRequest(value: unknown): AdapterRequest {
  const request = requireObject(value, "request");
  if (request.wire_version !== WIRE_VERSION) {
    throw invalid(`request.wire_version must be ${WIRE_VERSION}`);
  }
  if (request.action === "discover") {
    requireExactKeys(
      request,
      ["wire_version", "action", "endpoint", "headers", "authorization"],
      "request",
      ["headers", "authorization"],
    );
    return {
      wire_version: WIRE_VERSION,
      action: "discover",
      endpoint: requireString(request.endpoint, "request.endpoint"),
      ...optionalHeaders(request.headers),
      ...optionalAuthorization(request.authorization),
    };
  }
  if (request.action === "call") {
    requireExactKeys(
      request,
      [
        "wire_version",
        "action",
        "endpoint",
        "protocol_version",
        "headers",
        "authorization",
        "tool",
        "arguments",
      ],
      "request",
      ["headers", "authorization"],
    );
    return {
      wire_version: WIRE_VERSION,
      action: "call",
      endpoint: requireString(request.endpoint, "request.endpoint"),
      protocol_version: requireString(
        request.protocol_version,
        "request.protocol_version",
      ),
      ...optionalHeaders(request.headers),
      ...optionalAuthorization(request.authorization),
      tool: parseFrozenTool(request.tool),
      arguments: requireJsonObject(request.arguments, "request.arguments"),
    };
  }
  throw invalid("request.action must be 'discover' or 'call'");
}

function optionalHeaders(
  value: unknown,
): { readonly headers?: WireHeaders } {
  if (value === undefined) {
    return {};
  }
  const input = requireObject(value, "request.headers");
  const entries = Object.entries(input);
  if (entries.length > MAX_REQUEST_HEADERS) {
    throw invalid(`request.headers exceeds ${MAX_REQUEST_HEADERS} entries`);
  }
  const normalized = new Map<string, string>();
  let bytes = 0;
  for (const [name, unknownValue] of entries) {
    const lower = name.toLowerCase();
    if (!HEADER_TOKEN.test(name) || CLIENT_OWNED_HEADERS.has(lower)) {
      throw invalid(`request.headers contains forbidden name '${name}'`);
    }
    if (normalized.has(lower)) {
      throw invalid(`request.headers repeats '${name}' case-insensitively`);
    }
    if (typeof unknownValue !== "string") {
      throw invalid(`request.headers.${name} must be a string`);
    }
    try {
      new Headers([[name, unknownValue]]);
    } catch (error) {
      throw new AdapterProblem(
        "invalid_request",
        `request.headers.${name} is not a valid HTTP header value`,
        { code: "invalid_wire_request", cause: error },
      );
    }
    bytes += Buffer.byteLength(name, "utf8") + Buffer.byteLength(unknownValue, "utf8");
    if (bytes > MAX_REQUEST_HEADER_BYTES) {
      throw invalid(`request.headers exceeds ${MAX_REQUEST_HEADER_BYTES} bytes`);
    }
    normalized.set(lower, unknownValue);
  }
  return { headers: Object.fromEntries(normalized) };
}

function optionalAuthorization(
  value: unknown,
): { readonly authorization?: WireAuthorization } {
  if (value === undefined) {
    return {};
  }
  const authorization = requireObject(value, "request.authorization");
  requireExactKeys(
    authorization,
    ["scheme", "token"],
    "request.authorization",
  );
  if (authorization.scheme !== "bearer") {
    throw invalid("request.authorization.scheme must be 'bearer'");
  }
  const token = requireString(authorization.token, "request.authorization.token");
  if (
    Buffer.byteLength(token, "utf8") > MAX_AUTH_TOKEN_BYTES ||
    /[\s\u0000-\u001F\u007F]/u.test(token)
  ) {
    throw invalid("request.authorization.token is malformed or over limit");
  }
  return { authorization: { scheme: "bearer", token } };
}

function parseFrozenTool(value: unknown): FrozenMcpTool {
  const tool = requireObject(value, "request.tool");
  requireExactKeys(
    tool,
    ["name", "input_schema", "output_schema"],
    "request.tool",
    ["output_schema"],
  );
  return {
    name: requireString(tool.name, "request.tool.name"),
    input_schema: requireJsonObject(
      tool.input_schema,
      "request.tool.input_schema",
    ),
    ...(tool.output_schema === undefined
      ? {}
      : {
          output_schema: requireJsonObject(
            tool.output_schema,
            "request.tool.output_schema",
          ),
        }),
  };
}

function requireJsonObject(value: unknown, path: string): JsonObject {
  const object = requireObject(value, path);
  if (!isJsonValue(object)) {
    throw invalid(`${path} must contain only JSON values`);
  }
  return object as JsonObject;
}

function isJsonValue(value: unknown): value is JsonValue {
  const pending: unknown[] = [value];
  while (pending.length > 0) {
    const current = pending.pop();
    if (
      current === null ||
      typeof current === "string" ||
      typeof current === "boolean"
    ) {
      continue;
    }
    if (typeof current === "number") {
      if (!Number.isFinite(current)) {
        return false;
      }
      continue;
    }
    if (Array.isArray(current)) {
      pending.push(...current);
      continue;
    }
    if (typeof current === "object" && current !== null) {
      pending.push(...Object.values(current));
      continue;
    }
    return false;
  }
  return true;
}

function requireObject(value: unknown, path: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw invalid(`${path} must be a JSON object`);
  }
  return value as Record<string, unknown>;
}

function requireString(value: unknown, path: string): string {
  if (typeof value !== "string" || value.length === 0) {
    throw invalid(`${path} must be a non-empty string`);
  }
  return value;
}

function requireExactKeys(
  value: Record<string, unknown>,
  allowed: readonly string[],
  path: string,
  optional: readonly string[] = [],
): void {
  const allowedSet = new Set(allowed);
  for (const key of Object.keys(value)) {
    if (!allowedSet.has(key)) {
      throw invalid(`${path} contains unknown field '${key}'`);
    }
  }
  const optionalSet = new Set(optional);
  for (const key of allowed) {
    if (!optionalSet.has(key) && !(key in value)) {
      throw invalid(`${path} is missing required field '${key}'`);
    }
  }
}

function invalid(message: string): AdapterProblem {
  return new AdapterProblem("invalid_request", message, {
    code: "invalid_wire_request",
  });
}

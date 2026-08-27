import type {
  AdapterRequest,
  FrozenMcpTool,
  JsonObject,
  JsonValue,
  WireAuthorization,
} from "./contract.js";
import { AdapterProblem } from "./errors.js";
import { MAX_AUTH_TOKEN_BYTES, WIRE_VERSION } from "./limits.js";

export function parseAdapterRequest(value: unknown): AdapterRequest {
  const request = requireObject(value, "request");
  if (request.wire_version !== WIRE_VERSION) {
    throw invalid(`request.wire_version must be ${WIRE_VERSION}`);
  }
  if (request.action === "discover") {
    requireExactKeys(
      request,
      ["wire_version", "action", "endpoint", "authorization"],
      "request",
      ["authorization"],
    );
    return {
      wire_version: WIRE_VERSION,
      action: "discover",
      endpoint: requireString(request.endpoint, "request.endpoint"),
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
        "authorization",
        "tool",
        "arguments",
      ],
      "request",
      ["authorization"],
    );
    return {
      wire_version: WIRE_VERSION,
      action: "call",
      endpoint: requireString(request.endpoint, "request.endpoint"),
      ...optionalAuthorization(request.authorization),
      tool: parseFrozenTool(request.tool),
      arguments: requireJsonObject(request.arguments, "request.arguments"),
    };
  }
  throw invalid("request.action must be 'discover' or 'call'");
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

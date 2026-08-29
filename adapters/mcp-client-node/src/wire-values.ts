import type { JsonObject, JsonValue } from "./contract.js";
import { AdapterProblem } from "./errors.js";

export function requireBoundedString(
  value: unknown,
  path: string,
  maxBytes: number,
): string {
  const text = requireString(value, path);
  if (Buffer.byteLength(text, "utf8") > maxBytes) {
    throw invalid(`${path} exceeds ${maxBytes} bytes`);
  }
  return text;
}

export function requireJsonObject(value: unknown, path: string): JsonObject {
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

export function requireObject(
  value: unknown,
  path: string,
): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw invalid(`${path} must be a JSON object`);
  }
  return value as Record<string, unknown>;
}

export function requireString(value: unknown, path: string): string {
  if (typeof value !== "string" || value.length === 0) {
    throw invalid(`${path} must be a non-empty string`);
  }
  return value;
}

export function requireBoolean(value: unknown, path: string): boolean {
  if (typeof value !== "boolean") {
    throw invalid(`${path} must be a boolean`);
  }
  return value;
}

export function requireExactKeys(
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

export function invalid(message: string): AdapterProblem {
  return new AdapterProblem("invalid_request", message, {
    code: "invalid_wire_request",
  });
}

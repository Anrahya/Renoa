import { DriveApiError } from "./errors.js";

export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

export type JsonObject = { [key: string]: JsonValue };

export function isJsonObject(value: unknown): value is JsonObject {
  return isJsonValue(value) && !Array.isArray(value) && value !== null;
}

export function requireJsonObject(value: unknown, label: string): JsonObject {
  if (!isJsonObject(value)) {
    throw new DriveApiError(502, `Google Drive returned an invalid ${label}.`);
  }
  return value;
}

export function stringField(value: JsonObject, field: string): string | undefined {
  const fieldValue = value[field];
  return typeof fieldValue === "string" ? fieldValue : undefined;
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

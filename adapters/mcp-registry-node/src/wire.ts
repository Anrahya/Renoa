import type { RegistryRequest } from "./contract.js";
import { RegistryProblem } from "./errors.js";
import { MAX_QUERY_BYTES, WIRE_VERSION } from "./limits.js";

const REGISTRY_NAME = /^[a-zA-Z0-9.-]+\/[a-zA-Z0-9._-]+$/u;

export function parseRequest(value: unknown): RegistryRequest {
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
    if (/[\u0000-\u001F\u007F]/u.test(query)) {
      throw invalid("request.query must not contain control characters");
    }
    return { wire_version: WIRE_VERSION, action: "search", query };
  }
  if (request.action === "lookup") {
    exactKeys(
      request,
      ["wire_version", "action", "registry_name", "registry_version"],
      "request",
    );
    const registryName = string(request.registry_name, "request.registry_name");
    if (registryName.length > 200 || !REGISTRY_NAME.test(registryName)) {
      throw invalid("request.registry_name is not a valid MCP Registry server name");
    }
    const registryVersion = string(
      request.registry_version,
      "request.registry_version",
    );
    if (
      registryVersion.length === 0 ||
      Buffer.byteLength(registryVersion, "utf8") > 255 ||
      registryVersion === "latest" ||
      /[\u0000-\u001F\u007F/]/u.test(registryVersion)
    ) {
      throw invalid(
        "request.registry_version must be an exact 1-255 byte version, not 'latest'",
      );
    }
    return {
      wire_version: WIRE_VERSION,
      action: "lookup",
      registry_name: registryName,
      registry_version: registryVersion,
    };
  }
  throw invalid("request.action must be 'search' or 'lookup'");
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

function invalid(message: string): RegistryProblem {
  return new RegistryProblem("invalid_request", message, {
    code: "invalid_wire_request",
  });
}

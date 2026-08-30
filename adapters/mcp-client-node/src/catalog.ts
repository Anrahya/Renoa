import type { Tool } from "@modelcontextprotocol/client";
import { AjvJsonSchemaValidator } from "@modelcontextprotocol/client/validators/ajv";
import type {
  CatalogTool,
  DiscoveredCatalog,
  FrozenMcpTool,
  JsonObject,
  RejectedTool,
} from "./contract.js";
import { AdapterProblem, boundUtf8 } from "./errors.js";
import {
  MAX_CATALOG_BYTES,
  MAX_SCHEMA_DEPTH,
  MAX_SCHEMA_NODES,
  MAX_TOOL_DESCRIPTION_BYTES,
  MAX_TOOL_SCHEMA_BYTES,
} from "./limits.js";

const TOOL_NAME = /^[A-Za-z0-9_.-]{1,128}$/;
const HEADER_TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
const HEADER_TYPES = new Set(["string", "integer", "boolean"]);

const NON_REACHABLE_SCHEMA_KEYS = [
  "items",
  "prefixItems",
  "contains",
  "additionalProperties",
  "unevaluatedProperties",
  "unevaluatedItems",
  "propertyNames",
  "patternProperties",
  "dependentSchemas",
  "oneOf",
  "anyOf",
  "allOf",
  "not",
  "if",
  "then",
  "else",
  "$defs",
  "definitions",
] as const;

const OBJECT_OF_SCHEMAS_KEYS = new Set([
  "patternProperties",
  "dependentSchemas",
  "$defs",
  "definitions",
]);

export type InspectedTool =
  | { readonly accepted: CatalogTool }
  | { readonly rejected: RejectedTool };

type InputValidator = ReturnType<AjvJsonSchemaValidator["getValidator"]>;

interface NormalizedTool {
  readonly tool: CatalogTool;
  readonly validateInput: InputValidator;
}

export function inspectDiscoveredTool(
  tool: Tool,
  index: number,
): InspectedTool {
  try {
    return { accepted: normalizeTool(tool).tool };
  } catch (error) {
    const reason =
      error instanceof AdapterProblem
        ? error.message
        : "tool definition failed local validation";
    return {
      rejected: {
        index,
        ...(typeof tool.name === "string"
          ? { name: displayName(tool.name) }
          : {}),
        reason: boundUtf8(reason, 512),
      },
    };
  }
}

export function validateFrozenTool(
  tool: FrozenMcpTool,
  arguments_: JsonObject,
): CatalogTool {
  try {
    const normalized = normalizeTool({
      name: tool.name,
      inputSchema: tool.input_schema as Tool["inputSchema"],
      ...(tool.output_schema === undefined
        ? {}
        : { outputSchema: tool.output_schema }),
    });
    const result = normalized.validateInput(arguments_);
    if (!result.valid) {
      throw new AdapterProblem(
        "invalid_request",
        `MCP tool arguments do not match the frozen input schema: ${boundUtf8(result.errorMessage ?? "validation failed", 512)}`,
        { code: "invalid_tool_arguments" },
      );
    }
    return normalized.tool;
  } catch (error) {
    if (error instanceof AdapterProblem) {
      if (error.kind === "invalid_request") {
        throw error;
      }
      throw new AdapterProblem("invalid_request", error.message, {
        code: "invalid_frozen_tool",
        cause: error,
      });
    }
    throw error;
  }
}

export function finalizeCatalog(
  base: Omit<DiscoveredCatalog, "tools" | "rejected_tools">,
  accepted: readonly CatalogTool[],
  rejected: readonly RejectedTool[],
): DiscoveredCatalog {
  const sorted = [...accepted].sort((left, right) => {
    if (left.name < right.name) return -1;
    if (left.name > right.name) return 1;
    return 0;
  });
  for (let index = 1; index < sorted.length; index += 1) {
    if (sorted[index - 1]?.name === sorted[index]?.name) {
      throw new AdapterProblem(
        "protocol",
        `MCP catalog contains duplicate tool name '${sorted[index]?.name ?? "unknown"}'.`,
        { code: "duplicate_tool_name" },
      );
    }
  }

  const catalog: DiscoveredCatalog = {
    ...base,
    tools: sorted,
    rejected_tools: [...rejected],
  };
  if (jsonBytes(catalog) > MAX_CATALOG_BYTES) {
    throw resourceLimit(`catalog exceeds ${MAX_CATALOG_BYTES} encoded bytes`);
  }
  return catalog;
}

function normalizeTool(tool: Tool): NormalizedTool {
  if (!TOOL_NAME.test(tool.name)) {
    throw new AdapterProblem(
      "protocol",
      "tool name must be 1-128 ASCII letters, digits, '_', '-', or '.'",
      { code: "invalid_tool_name" },
    );
  }
  const description = tool.description ?? "";
  if (Buffer.byteLength(description, "utf8") > MAX_TOOL_DESCRIPTION_BYTES) {
    throw resourceLimit(
      `tool description exceeds ${MAX_TOOL_DESCRIPTION_BYTES} bytes`,
    );
  }

  const inputSchema = tool.inputSchema as JsonObject;
  if (inputSchema.type !== "object") {
    throw new AdapterProblem(
      "protocol",
      "tool input schema root must have type 'object'",
      {
        code: "invalid_input_schema_root",
      },
    );
  }
  assertJsonTreeBounded(inputSchema);
  assertSchemaSize(inputSchema, "input schema");
  const modelInputSchema = stripAndValidateHeaderAnnotations(inputSchema);
  let validateInput: InputValidator;
  try {
    validateInput = new AjvJsonSchemaValidator().getValidator(inputSchema);
  } catch (error) {
    throw new AdapterProblem(
      "protocol",
      `tool input schema cannot be compiled: ${boundUtf8(error instanceof Error ? error.message : String(error), 512)}`,
      { code: "invalid_input_schema", cause: error },
    );
  }

  const outputSchema = tool.outputSchema as JsonObject | undefined;
  if (outputSchema !== undefined) {
    assertJsonTreeBounded(outputSchema);
    assertSchemaSize(outputSchema, "output schema");
  }

  return {
    tool: {
      name: tool.name,
      description,
      input_schema: inputSchema,
      model_input_schema: modelInputSchema,
      ...(outputSchema === undefined ? {} : { output_schema: outputSchema }),
    },
    validateInput,
  };
}

function stripAndValidateHeaderAnnotations(
  inputSchema: JsonObject,
): JsonObject {
  validateHeaderAnnotations(inputSchema);

  const modelSchema = structuredClone(inputSchema);
  const pending: unknown[] = [modelSchema];
  while (pending.length > 0) {
    const current = pending.pop();
    if (Array.isArray(current)) {
      pending.push(...current);
      continue;
    }
    if (!isObject(current)) {
      continue;
    }
    delete current["x-mcp-header"];
    pending.push(...Object.values(current));
  }
  return modelSchema;
}

function validateHeaderAnnotations(inputSchema: JsonObject): void {
  interface Frame {
    readonly value: unknown;
    readonly path: readonly string[];
    readonly reachable: boolean;
  }

  const pending: Frame[] = [{ value: inputSchema, path: [], reachable: true }];
  const names = new Map<string, string>();
  while (pending.length > 0) {
    const frame = pending.pop();
    if (frame === undefined || !isObject(frame.value)) {
      continue;
    }
    const schema = frame.value;
    if (Object.hasOwn(schema, "x-mcp-header")) {
      const header = schema["x-mcp-header"];
      const location =
        frame.path.length === 0 ? "<root>" : frame.path.join(".");
      if (!frame.reachable || frame.path.length === 0) {
        throw invalidHeader(
          `${location}: annotation is not reachable only through properties`,
        );
      }
      if (
        typeof header !== "string" ||
        header.length === 0 ||
        !HEADER_TOKEN.test(header)
      ) {
        throw invalidHeader(
          `${location}: header name must be a non-empty RFC 9110 token`,
        );
      }
      if (typeof schema.type !== "string" || !HEADER_TYPES.has(schema.type)) {
        throw invalidHeader(
          `${location}: annotated property must be string, integer, or boolean`,
        );
      }
      const lower = header.toLowerCase();
      const previous = names.get(lower);
      if (previous !== undefined) {
        throw invalidHeader(
          `header '${header}' duplicates '${previous}' case-insensitively`,
        );
      }
      names.set(lower, header);
    }

    if (isObject(schema.properties)) {
      for (const [name, child] of Object.entries(schema.properties)) {
        pending.push({
          value: child,
          path: [...frame.path, name],
          reachable: frame.reachable,
        });
      }
    }
    for (const key of NON_REACHABLE_SCHEMA_KEYS) {
      const value = schema[key];
      if (value === undefined) {
        continue;
      }
      const children = schemaChildren(key, value);
      for (const child of children) {
        pending.push({
          value: child,
          path: [...frame.path, `<${key}>`],
          reachable: false,
        });
      }
    }
  }
}

function schemaChildren(key: string, value: unknown): readonly unknown[] {
  if (Array.isArray(value)) {
    return value;
  }
  if (OBJECT_OF_SCHEMAS_KEYS.has(key) && isObject(value)) {
    return Object.values(value);
  }
  return [value];
}

function assertJsonTreeBounded(value: JsonObject): void {
  const pending: { readonly value: unknown; readonly depth: number }[] = [
    { value, depth: 0 },
  ];
  let nodes = 0;
  while (pending.length > 0) {
    const frame = pending.pop();
    if (frame === undefined) {
      continue;
    }
    if (frame.depth > MAX_SCHEMA_DEPTH) {
      throw resourceLimit(`schema exceeds nesting depth ${MAX_SCHEMA_DEPTH}`);
    }
    if (!isObject(frame.value) && !Array.isArray(frame.value)) {
      continue;
    }
    nodes += 1;
    if (nodes > MAX_SCHEMA_NODES) {
      throw resourceLimit(`schema exceeds ${MAX_SCHEMA_NODES} container nodes`);
    }
    const children = Array.isArray(frame.value)
      ? frame.value
      : Object.values(frame.value);
    for (const child of children) {
      pending.push({ value: child, depth: frame.depth + 1 });
    }
  }
}

function assertSchemaSize(value: JsonObject, label: string): void {
  if (jsonBytes(value) > MAX_TOOL_SCHEMA_BYTES) {
    throw resourceLimit(
      `${label} exceeds ${MAX_TOOL_SCHEMA_BYTES} encoded bytes`,
    );
  }
}

function jsonBytes(value: unknown): number {
  return Buffer.byteLength(JSON.stringify(value), "utf8");
}

function displayName(value: string): string {
  return boundUtf8(value.replace(/[^\x20-\x7E]/g, "?"), 128);
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function invalidHeader(reason: string): AdapterProblem {
  return new AdapterProblem("protocol", `invalid x-mcp-header: ${reason}`, {
    code: "invalid_x_mcp_header",
  });
}

function resourceLimit(message: string): AdapterProblem {
  return new AdapterProblem("resource_limit", message, {
    code: "resource_limit",
  });
}

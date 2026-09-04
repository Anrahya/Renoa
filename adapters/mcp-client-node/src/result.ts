import type { JsonValue, WireToolContent, WireToolResult } from "./contract.js";
import { AdapterProblem } from "./errors.js";
import {
  MAX_CONTENT_BLOCKS,
  MAX_STRUCTURED_CONTENT_BYTES,
  MAX_TOOL_RESULT_BYTES,
} from "./limits.js";

const IMAGE_MEDIA_TYPE = /^image\/[A-Za-z0-9][A-Za-z0-9!#$&^_.+-]{0,126}$/;
const CANONICAL_BASE64 =
  /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;

export function projectToolResult(
  value: unknown,
  outputSchemaPresent: boolean,
): WireToolResult {
  const result = requireObject(value, "tool result");
  if (result.resultType === "input_required") {
    throw new AdapterProblem(
      "unsupported_result",
      "MCP tool requested another round of input, which this adapter revision does not support.",
      {
        code: "input_required",
        partialChangesPossible: true,
      },
    );
  }
  if (!Array.isArray(result.content)) {
    throw invalidResult("MCP complete result is missing its content array");
  }
  if (result.content.length > MAX_CONTENT_BLOCKS) {
    throw resourceLimit(
      `tool result has more than ${MAX_CONTENT_BLOCKS} content blocks`,
    );
  }

  if (result.isError !== undefined && typeof result.isError !== "boolean") {
    throw invalidResult("MCP isError field is not boolean");
  }
  const isError = result.isError === true;
  const hasStructuredContent = Object.hasOwn(result, "structuredContent");
  if (outputSchemaPresent && !isError && !hasStructuredContent) {
    throw invalidResult(
      "tool declared an output schema but returned no structured content",
    );
  }
  if (result.content.length === 0) {
    throw new AdapterProblem(
      "unsupported_result",
      "MCP structured-only results are not supported in this adapter revision.",
      {
        code: "structured_only_result",
        partialChangesPossible: true,
      },
    );
  }

  const content: WireToolContent[] = [];
  for (const block of result.content) {
    const item = requireObject(block, "tool result content block");
    if (item.type === "text") {
      if (typeof item.text !== "string") {
        throw invalidResult("MCP text result contains a non-string value");
      }
      content.push({ type: "text", text: item.text });
      continue;
    }
    if (item.type === "image") {
      if (typeof item.data !== "string" || !isCanonicalBase64(item.data)) {
        throw invalidResult("MCP image result contains invalid base64 data");
      }
      if (
        typeof item.mimeType !== "string" ||
        !IMAGE_MEDIA_TYPE.test(item.mimeType)
      ) {
        throw invalidResult(
          "MCP image result contains an invalid image media type",
        );
      }
      content.push({
        type: "image",
        data: item.data,
        mime_type: item.mimeType,
      });
      continue;
    }
    throw new AdapterProblem(
      "unsupported_result",
      `MCP result content type '${displayType(item.type)}' is not supported.`,
      {
        code: "unsupported_content_type",
        partialChangesPossible: true,
      },
    );
  }

  const structuredContent = result.structuredContent;
  if (hasStructuredContent) {
    if (!isJsonValue(structuredContent)) {
      throw invalidResult("MCP structured content is not a JSON value");
    }
    if (jsonBytes(structuredContent) > MAX_STRUCTURED_CONTENT_BYTES) {
      throw resourceLimit(
        `structured content exceeds ${MAX_STRUCTURED_CONTENT_BYTES} encoded bytes`,
      );
    }
  }

  const projected: WireToolResult = {
    content,
    structured_content: hasStructuredContent
      ? { present: true, value: structuredContent as JsonValue }
      : { present: false },
    is_error: isError,
  };
  if (jsonBytes(projected) > MAX_TOOL_RESULT_BYTES) {
    throw resourceLimit(
      `tool result exceeds ${MAX_TOOL_RESULT_BYTES} encoded bytes`,
    );
  }
  return projected;
}

function isCanonicalBase64(value: string): boolean {
  if (!CANONICAL_BASE64.test(value)) {
    return false;
  }
  return Buffer.from(value, "base64").toString("base64") === value;
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

function requireObject(value: unknown, label: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw invalidResult(`${label} must be a JSON object`);
  }
  return value as Record<string, unknown>;
}

function displayType(value: unknown): string {
  return typeof value === "string" ? value.slice(0, 64) : typeof value;
}

function jsonBytes(value: unknown): number {
  return Buffer.byteLength(JSON.stringify(value), "utf8");
}

function invalidResult(message: string): AdapterProblem {
  return new AdapterProblem("invalid_result", message, {
    code: "invalid_tool_result",
    partialChangesPossible: true,
  });
}

function resourceLimit(message: string): AdapterProblem {
  return new AdapterProblem("resource_limit", message, {
    code: "resource_limit",
    partialChangesPossible: true,
  });
}

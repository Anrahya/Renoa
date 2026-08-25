import type {
  JsonValue,
  WireModelRequest,
  WireModelResponse,
} from "./contract.js";
import type { Api, AssistantMessage, Context, Message, Tool } from "./upstream/types.js";
import { isContextOverflow } from "./upstream/overflow.js";

export function parseWireModelRequest(value: unknown): WireModelRequest {
  if (!isRecord(value)) {
    throw invalidRequest("model request must be a JSON object");
  }
  if (typeof value.system_prompt !== "string") {
    throw invalidRequest("model request is missing system_prompt");
  }
  if (!Array.isArray(value.messages)) {
    throw invalidRequest("model request is missing messages");
  }
  if (!Array.isArray(value.tools)) {
    throw invalidRequest("model request is missing tools");
  }
  return {
    system_prompt: value.system_prompt,
    messages: value.messages.map((message, index) => parseMessage(message, index)),
    tools: value.tools.map((tool, index) => parseTool(tool, index)),
  };
}

function parseMessage(value: unknown, index: number): WireModelRequest["messages"][number] {
  if (!isRecord(value)) {
    throw invalidRequest(`messages[${index}] is malformed`);
  }
  if (value.role === "user") {
    return { role: "user", content: parseContentList(value.content, `messages[${index}].content`) };
  }
  if (value.role === "assistant") {
    return parseAssistantMessage(value, index);
  }
  if (value.role === "tool") {
    return { role: "tool", result: parseToolResult(value.result, `messages[${index}].result`) };
  }
  throw invalidRequest(`messages[${index}] is malformed`);
}

function parseAssistantMessage(
  value: Record<string, unknown>,
  index: number,
): Extract<WireModelRequest["messages"][number], { role: "assistant" }> {
  if (value.stop_reason !== "stop" && value.stop_reason !== "tool_use" && value.stop_reason !== "length") {
    throw invalidRequest(`messages[${index}].stop_reason is malformed`);
  }
  const metadata = parseMetadata(value.metadata, `messages[${index}].metadata`);
  if (
    metadata.api !== "openai-completions" &&
    metadata.api !== "openai-responses" &&
    metadata.api !== "anthropic-messages"
  ) {
    throw invalidRequest(`messages[${index}].metadata.api is malformed`);
  }
  if (metadata.provider == null || metadata.provider === "") {
    throw invalidRequest(`messages[${index}].metadata.provider is malformed`);
  }
  if (metadata.model == null || metadata.model === "") {
    throw invalidRequest(`messages[${index}].metadata.model is malformed`);
  }
  return {
    role: "assistant",
    content: parseAssistantContentList(value.content, `messages[${index}].content`),
    stop_reason: value.stop_reason,
    usage: parseUsage(value.usage, `messages[${index}].usage`),
    metadata,
  };
}

function parseToolResult(value: unknown, path: string): Extract<WireModelRequest["messages"][number], { role: "tool" }>["result"] {
  if (!isRecord(value)) {
    throw invalidRequest(`${path} is malformed`);
  }
  if (typeof value.call_id !== "string") {
    throw invalidRequest(`${path}.call_id is malformed`);
  }
  if (typeof value.name !== "string") {
    throw invalidRequest(`${path}.name is malformed`);
  }
  if (typeof value.is_error !== "boolean") {
    throw invalidRequest(`${path}.is_error is malformed`);
  }
  if (value.details !== null && !isJsonValue(value.details)) {
    throw invalidRequest(`${path}.details is malformed`);
  }
  return {
    call_id: value.call_id,
    name: value.name,
    content: parseContentList(value.content, `${path}.content`),
    details: value.details === null ? null : (value.details as JsonValue),
    is_error: value.is_error,
  };
}

function parseTool(value: unknown, index: number): WireModelRequest["tools"][number] {
  if (!isRecord(value)) {
    throw invalidRequest(`tools[${index}] is malformed`);
  }
  if (typeof value.name !== "string") {
    throw invalidRequest(`tools[${index}].name is malformed`);
  }
  if (typeof value.description !== "string") {
    throw invalidRequest(`tools[${index}].description is malformed`);
  }
  if (!isRecord(value.input_schema) || !isJsonValue(value.input_schema)) {
    throw invalidRequest(`tools[${index}].input_schema must be a JSON object`);
  }
  return {
    name: value.name,
    description: value.description,
    input_schema: value.input_schema as JsonValue,
  };
}

function parseContentList(value: unknown, path: string): WireModelRequest["messages"][number] extends never ? never : readonly (
  | { type: "text"; text: string }
  | { type: "image"; data: string; mime_type: string }
)[] {
  if (!Array.isArray(value)) {
    throw invalidRequest(`${path} is malformed`);
  }
  return value.map((entry, index) => parseContent(entry, `${path}[${index}]`));
}

function parseContent(
  value: unknown,
  path: string,
): { type: "text"; text: string } | { type: "image"; data: string; mime_type: string } {
  if (!isRecord(value)) {
    throw invalidRequest(`${path} is malformed`);
  }
  if (value.type === "text") {
    if (typeof value.text !== "string") {
      throw invalidRequest(`${path}.text is malformed`);
    }
    return { type: "text", text: value.text };
  }
  if (value.type === "image") {
    if (typeof value.data !== "string" || typeof value.mime_type !== "string") {
      throw invalidRequest(`${path} image fields are malformed`);
    }
    return { type: "image", data: value.data, mime_type: value.mime_type };
  }
  throw invalidRequest(`${path} is malformed`);
}

function parseAssistantContentList(value: unknown, path: string): WireModelResponse["content"] {
  if (!Array.isArray(value)) {
    throw invalidRequest(`${path} is malformed`);
  }
  return value.map((entry, index) => parseAssistantContent(entry, `${path}[${index}]`));
}

function parseAssistantContent(value: unknown, path: string): WireModelResponse["content"][number] {
  if (!isRecord(value)) {
    throw invalidRequest(`${path} is malformed`);
  }
  if (value.type === "text") {
    if (typeof value.text !== "string") {
      throw invalidRequest(`${path}.text is malformed`);
    }
    return {
      type: "text",
      text: value.text,
      ...(typeof value.signature === "string" ? { signature: value.signature } : value.signature === undefined ? {} : invalidField(`${path}.signature`)),
    };
  }
  if (value.type === "reasoning") {
    if (
      typeof value.text !== "string" ||
      (value.redacted !== undefined && typeof value.redacted !== "boolean")
    ) {
      throw invalidRequest(`${path} reasoning fields are malformed`);
    }
    return {
      type: "reasoning",
      text: value.text,
      redacted: value.redacted ?? false,
      ...(typeof value.signature === "string" ? { signature: value.signature } : value.signature === undefined ? {} : invalidField(`${path}.signature`)),
    };
  }
  if (value.type === "tool_call") {
    if (typeof value.id !== "string" || typeof value.name !== "string") {
      throw invalidRequest(`${path} tool_call fields are malformed`);
    }
    if (!isRecord(value.arguments) || !isJsonValue(value.arguments)) {
      throw invalidRequest(`${path}.arguments must be a JSON object`);
    }
    return {
      type: "tool_call",
      id: value.id,
      name: value.name,
      arguments: value.arguments as JsonValue,
      ...(typeof value.thought_signature === "string"
        ? { thought_signature: value.thought_signature }
        : value.thought_signature === undefined
          ? {}
          : invalidField(`${path}.thought_signature`)),
    };
  }
  throw invalidRequest(`${path} is malformed`);
}

function parseUsage(value: unknown, path: string): import("./contract.js").WireUsage | null {
  if (value === null) {
    return null;
  }
  if (!isRecord(value)) {
    throw invalidRequest(`${path} is malformed`);
  }
  return {
    input: finiteNumber(value.input, `${path}.input`),
    output: finiteNumber(value.output, `${path}.output`),
    cache_read: finiteNumber(value.cache_read, `${path}.cache_read`),
    cache_write: finiteNumber(value.cache_write, `${path}.cache_write`),
  };
}

function parseMetadata(value: unknown, path: string): import("./contract.js").WireMetadata {
  if (!isRecord(value)) {
    throw invalidRequest(`${path} is malformed`);
  }
  return {
    ...(optionalString(value.api, `${path}.api`) ? { api: value.api as string | null } : {}),
    ...(optionalString(value.provider, `${path}.provider`) ? { provider: value.provider as string | null } : {}),
    ...(optionalString(value.model, `${path}.model`) ? { model: value.model as string | null } : {}),
    ...(optionalString(value.response_model, `${path}.response_model`)
      ? { response_model: value.response_model as string | null }
      : {}),
    ...(optionalString(value.response_id, `${path}.response_id`)
      ? { response_id: value.response_id as string | null }
      : {}),
    ...(optionalString(value.raw_stop_reason, `${path}.raw_stop_reason`)
      ? { raw_stop_reason: value.raw_stop_reason as string | null }
      : {}),
  };
}

function optionalString(value: unknown, path: string): boolean {
  if (value === undefined) {
    return false;
  }
  if (value !== null && typeof value !== "string") {
    throw invalidRequest(`${path} is malformed`);
  }
  return true;
}

function finiteNumber(value: unknown, path: string): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw invalidRequest(`${path} is malformed`);
  }
  return value;
}

function isJsonValue(value: unknown): value is JsonValue {
  if (value === null || typeof value === "boolean" || typeof value === "string") {
    return true;
  }
  if (typeof value === "number") {
    return Number.isFinite(value);
  }
  if (Array.isArray(value)) {
    return value.every(isJsonValue);
  }
  if (isRecord(value)) {
    return Object.values(value).every(isJsonValue);
  }
  return false;
}

function invalidField(path: string): never {
  throw invalidRequest(`${path} is malformed`);
}

export function toContext(request: WireModelRequest): Context {
  return {
    systemPrompt: request.system_prompt,
    messages: request.messages.map(toMessage),
    tools: request.tools.map(toTool),
  };
}

export function fromAssistant(message: AssistantMessage, contextWindow: number): WireModelResponse {
  if (message.stopReason === "error" || message.stopReason === "aborted") {
    const overflow =
      message.stopReason === "error" &&
      message.usage.output === 0 &&
      message.content.every((content) => content.type === "text" && content.text.length === 0) &&
      isContextOverflow(message, contextWindow);
    if (overflow) {
      throw Object.assign(new Error(message.errorMessage ?? "context window exceeded"), {
        categoryHint: "context_window_exceeded",
      });
    }
    throw new Error(message.errorMessage ?? `model stopped with ${message.stopReason}`);
  }
  if (message.stopReason === "pending") {
    throw new Error(`model bridge does not support ${message.stopReason} responses`);
  }
  return {
    content: message.content.map((content) => {
      switch (content.type) {
        case "text":
          return {
            type: "text" as const,
            text: content.text,
            ...(content.textSignature === undefined ? {} : { signature: content.textSignature }),
          };
        case "thinking":
          return {
            type: "reasoning" as const,
            text: content.thinking,
            ...(content.thinkingSignature === undefined
              ? {}
              : { signature: content.thinkingSignature }),
            redacted: content.redacted ?? false,
          };
        case "toolCall":
          return {
            type: "tool_call" as const,
            id: content.id,
            name: content.name,
            arguments: content.arguments as JsonValue,
            ...(content.thoughtSignature === undefined
              ? {}
              : { thought_signature: content.thoughtSignature }),
          };
      }
    }),
    stop_reason: stopReason(message),
    usage: {
      input: message.usage.input,
      output: message.usage.output,
      cache_read: message.usage.cacheRead,
      cache_write: message.usage.cacheWrite,
    },
    metadata: {
      api: message.api,
      provider: message.provider,
      model: message.model,
      ...(message.responseModel === undefined ? {} : { response_model: message.responseModel }),
      ...(message.responseId === undefined ? {} : { response_id: message.responseId }),
      ...(message.rawStopReason === undefined ? {} : { raw_stop_reason: message.rawStopReason }),
    },
  };
}

function toMessage(message: WireModelRequest["messages"][number]): Message {
  switch (message.role) {
    case "user":
      return { role: "user", content: message.content.map(toContent), timestamp: 0 };
    case "assistant":
      return {
        role: "assistant",
        content: message.content.map(toAssistantContent),
        api: requiredApi(message.metadata),
        provider: requiredMetadata(message.metadata, "provider"),
        model: requiredMetadata(message.metadata, "model"),
        ...(message.metadata.response_model == null
          ? {}
          : { responseModel: message.metadata.response_model }),
        ...(message.metadata.response_id == null ? {} : { responseId: message.metadata.response_id }),
        usage: emptyUsage(),
        stopReason: message.stop_reason === "tool_use" ? "toolUse" : message.stop_reason,
        ...(message.metadata.raw_stop_reason == null
          ? {}
          : { rawStopReason: message.metadata.raw_stop_reason }),
        timestamp: 0,
      };
    case "tool":
      return {
        role: "toolResult",
        toolCallId: message.result.call_id,
        toolName: message.result.name,
        content: message.result.content.map(toContent),
        ...(message.result.details === null ? {} : { details: message.result.details }),
        isError: message.result.is_error,
        timestamp: 0,
      };
  }
}

function toContent(content: { type: "text"; text: string } | { type: "image"; data: string; mime_type: string }) {
  return content.type === "text"
    ? { type: "text" as const, text: content.text }
    : { type: "image" as const, data: content.data, mimeType: content.mime_type };
}

function toAssistantContent(content: WireModelResponse["content"][number]) {
  switch (content.type) {
    case "text":
      return {
        type: "text" as const,
        text: content.text,
        ...(content.signature === undefined ? {} : { textSignature: content.signature }),
      };
    case "reasoning":
      return {
        type: "thinking" as const,
        thinking: content.text,
        ...(content.signature === undefined ? {} : { thinkingSignature: content.signature }),
        redacted: content.redacted,
      };
    case "tool_call":
      return {
        type: "toolCall" as const,
        id: content.id,
        name: content.name,
        arguments: objectArguments(content.arguments),
        ...(content.thought_signature === undefined
          ? {}
          : { thoughtSignature: content.thought_signature }),
      };
  }
}

function emptyUsage(): AssistantMessage["usage"] {
  return {
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens: 0,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  };
}

function requiredApi(metadata: { api?: string | null }): Api {
  const value = requiredMetadata(metadata, "api");
  if (value === "openai-completions" || value === "openai-responses" || value === "anthropic-messages") {
    return value;
  }
  throw invalidRequest(`assistant history has unsupported api metadata: ${value}`);
}

function requiredMetadata(
  metadata: { api?: string | null; provider?: string | null; model?: string | null },
  name: "api" | "provider" | "model",
): string {
  const value = metadata[name];
  if (value == null || value === "") {
    throw invalidRequest(`assistant history is missing ${name} metadata`);
  }
  return value;
}

function objectArguments(value: JsonValue): Record<string, JsonValue> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw invalidRequest("tool-call arguments must be a JSON object");
  }
  return value;
}

function toTool(tool: { name: string; description: string; input_schema: JsonValue }): Tool {
  return {
    name: tool.name,
    description: tool.description,
    parameters: tool.input_schema,
  };
}

function stopReason(message: AssistantMessage): WireModelResponse["stop_reason"] {
  switch (message.stopReason) {
    case "stop":
      return "stop";
    case "toolUse":
      return "tool_use";
    case "length":
      return "length";
    case "pending":
    case "error":
    case "aborted":
      throw new Error(`unsupported model stop reason: ${message.stopReason}`);
  }
}

function invalidRequest(message: string): Error {
  return Object.assign(new Error(message), { categoryHint: "invalid_request" });
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

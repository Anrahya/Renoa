import type {
  AssistantMessage,
  Context,
  Message,
  Tool,
} from "@earendil-works/pi-ai";
import { isContextOverflow } from "@earendil-works/pi-ai";

import type { ModelRuntime } from "./model-runtime.js";

type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

export interface WireModelRequest {
  readonly system_prompt: string;
  readonly messages: readonly WireMessage[];
  readonly tools: readonly WireTool[];
}

type WireMessage =
  | {
      readonly role: "user";
      readonly content: readonly WireContent[];
    }
  | {
      readonly role: "assistant";
      readonly content: readonly WireAssistantContent[];
      readonly stop_reason: "stop" | "tool_use" | "length";
      readonly usage: WireUsage | null;
      readonly metadata: WireMetadata;
    }
  | {
      readonly role: "tool";
      readonly result: {
        readonly call_id: string;
        readonly name: string;
        readonly content: readonly WireContent[];
        readonly details: JsonValue | null;
        readonly is_error: boolean;
      };
    };

type WireContent =
  | { readonly type: "text"; readonly text: string }
  | { readonly type: "image"; readonly data: string; readonly mime_type: string };

interface WireTool {
  readonly name: string;
  readonly description: string;
  readonly input_schema: JsonValue;
}

interface WireUsage {
  readonly input: number;
  readonly output: number;
  readonly cache_read: number;
  readonly cache_write: number;
}

interface WireMetadata {
  readonly api?: string | null;
  readonly provider?: string | null;
  readonly model?: string | null;
  readonly response_model?: string | null;
  readonly response_id?: string | null;
  readonly raw_stop_reason?: string | null;
}

export interface WireModelResponse {
  readonly content: readonly WireAssistantContent[];
  readonly stop_reason: "stop" | "tool_use" | "length";
  readonly usage: {
    readonly input: number;
    readonly output: number;
    readonly cache_read: number;
    readonly cache_write: number;
  };
  readonly metadata: {
    readonly api: string;
    readonly provider: string;
    readonly model: string;
    readonly response_model?: string;
    readonly response_id?: string;
    readonly raw_stop_reason?: string;
  };
}

type WireAssistantContent =
  | { readonly type: "text"; readonly text: string; readonly signature?: string }
  | {
      readonly type: "reasoning";
      readonly text: string;
      readonly signature?: string;
      readonly redacted: boolean;
    }
  | {
      readonly type: "tool_call";
      readonly id: string;
      readonly name: string;
      readonly arguments: JsonValue;
      readonly thought_signature?: string;
    };

type ModelBinding = Pick<ModelRuntime, "model" | "streamFn">;

export type ModelInvocationErrorKind = "context_window_exceeded";

export class ModelInvocationError extends Error {
  readonly kind: ModelInvocationErrorKind | undefined;

  constructor(message: string, kind?: ModelInvocationErrorKind) {
    super(message);
    this.name = "ModelInvocationError";
    this.kind = kind;
  }
}

export async function invokeModel(
  request: WireModelRequest,
  runtime: ModelBinding,
  maxOutputTokens?: number,
): Promise<WireModelResponse> {
  const stream = await runtime.streamFn(
    runtime.model,
    toContext(request),
    maxOutputTokens === undefined ? undefined : { maxTokens: maxOutputTokens },
  );
  return fromAssistant(await stream.result(), runtime.model.contextWindow);
}

function toContext(request: WireModelRequest): Context {
  return {
    systemPrompt: request.system_prompt,
    messages: request.messages.map(toMessage),
    tools: request.tools.map(toTool),
  };
}

function toMessage(message: WireMessage): Message {
  switch (message.role) {
    case "user":
      return { role: "user", content: message.content.map(toContent), timestamp: 0 };
    case "assistant":
      return {
        role: "assistant",
        content: message.content.map(toAssistantContent),
        api: requiredMetadata(message.metadata, "api"),
        provider: requiredMetadata(message.metadata, "provider"),
        model: requiredMetadata(message.metadata, "model"),
        ...(message.metadata.response_model == null
          ? {}
          : { responseModel: message.metadata.response_model }),
        ...(message.metadata.response_id == null
          ? {}
          : { responseId: message.metadata.response_id }),
        usage: emptyUsage(),
        stopReason: toPiStopReason(message.stop_reason),
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

function toContent(content: WireContent) {
  return content.type === "text"
    ? { type: "text" as const, text: content.text }
    : { type: "image" as const, data: content.data, mimeType: content.mime_type };
}

function toAssistantContent(content: WireAssistantContent) {
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
  // Historical usage describes the original prefix, not a checkpointed replay.
  // Zero forces Pi's context guard to estimate the request that is actually sent.
  return {
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheWrite: 0,
    totalTokens: 0,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  };
}

function toPiStopReason(reason: "stop" | "tool_use" | "length") {
  return reason === "tool_use" ? ("toolUse" as const) : reason;
}

function requiredMetadata(metadata: WireMetadata, name: "api" | "provider" | "model"): string {
  const value = metadata[name];
  if (value == null || value === "") {
    throw new Error(`assistant history is missing ${name} metadata`);
  }
  return value;
}

function objectArguments(value: JsonValue): Record<string, JsonValue> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error("tool-call arguments must be a JSON object");
  }
  return value;
}

function toTool(tool: WireTool): Tool {
  return {
    name: tool.name,
    description: tool.description,
    parameters: tool.input_schema,
  } as Tool;
}

function fromAssistant(message: AssistantMessage, contextWindow: number): WireModelResponse {
  if (message.stopReason === "error" || message.stopReason === "aborted") {
    const kind =
      message.stopReason === "error" &&
      message.usage.output === 0 &&
      message.content.every((content) => content.type === "text" && content.text.length === 0) &&
      isContextOverflow(message, contextWindow)
        ? "context_window_exceeded"
        : undefined;
    throw new ModelInvocationError(
      message.errorMessage ?? `model stopped with ${message.stopReason}`,
      kind,
    );
  }
  if (message.stopReason === "pending" || message.stopReason === "deferred") {
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
    case "deferred":
      throw new Error(`unsupported model stop reason: ${message.stopReason}`);
  }
}

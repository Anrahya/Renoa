import type {
  AssistantMessage,
  AssistantMessageEvent,
  Context,
  Message,
  ProviderResponse,
  Tool,
  ModelThinkingLevel,
} from "@earendil-works/pi-ai";
import { isContextOverflow, ModelsError } from "@earendil-works/pi-ai";

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

export type WireStreamRecord =
  | { readonly event: "provider_request"; readonly payload: JsonValue }
  | {
      readonly event: "provider_response";
      readonly status: number;
      readonly headers: Readonly<Record<string, string>>;
    }
  | {
      readonly event: "content_delta";
      readonly content_index: number;
      readonly delta: WireStreamDelta;
    }
  | { readonly event: "completed"; readonly response: WireModelResponse }
  | {
      readonly event: "error";
      readonly error: string;
      readonly error_kind?: ModelInvocationErrorKind;
    };

type WireStreamDelta =
  | { readonly type: "text"; readonly text: string }
  | { readonly type: "reasoning"; readonly text: string }
  | { readonly type: "tool_call_start"; readonly id: string; readonly name: string }
  | { readonly type: "tool_call_arguments"; readonly json_delta: string };

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

type ModelBinding = Pick<ModelRuntime, "authenticate" | "model" | "streamFn"> & {
  readonly reasoningLevel?: ModelThinkingLevel;
};

export type ModelInvocationErrorKind =
  | "context_window_exceeded"
  | "authentication_failed";

export class ModelInvocationError extends Error {
  readonly kind: ModelInvocationErrorKind | undefined;

  constructor(message: string, kind?: ModelInvocationErrorKind) {
    super(message);
    this.name = "ModelInvocationError";
    this.kind = kind;
  }
}

export async function streamModel(
  request: WireModelRequest,
  runtime: ModelBinding,
  maxOutputTokens: number | undefined,
  emit: (record: WireStreamRecord) => void | Promise<void>,
): Promise<void> {
  const options = {
    ...(maxOutputTokens === undefined ? {} : { maxTokens: maxOutputTokens }),
    ...(runtime.reasoningLevel === undefined || runtime.reasoningLevel === "off"
      ? {}
      : { reasoning: runtime.reasoningLevel }),
    onPayload: async (payload: unknown) => {
      await emit({ event: "provider_request", payload: diagnosticValue(payload) });
      return undefined;
    },
    onResponse: async (response: ProviderResponse) => {
      await emit({
        event: "provider_response",
        status: response.status,
        headers: redactedHeaders(response.headers),
      });
    },
  };
  try {
    await runtime.authenticate();
  } catch (error) {
    if (
      error instanceof ModelsError &&
      (error.code === "auth" || error.code === "oauth")
    ) {
      throw new ModelInvocationError(error.message, "authentication_failed");
    }
    throw error;
  }
  const stream = await runtime.streamFn(runtime.model, toContext(request), options);
  for await (const event of stream) {
    const record = contentDelta(event);
    if (record !== undefined) {
      await emit(record);
      continue;
    }
    if (event.type === "done") {
      await emit({
        event: "completed",
        response: fromAssistant(event.message, runtime.model.contextWindow),
      });
      return;
    }
    if (event.type === "error") {
      fromAssistant(event.error, runtime.model.contextWindow);
      throw new Error("Pi returned an invalid successful error event");
    }
  }
  throw new Error("Pi model stream closed without a terminal event");
}

function diagnosticValue(value: unknown): JsonValue {
  const encoded = JSON.stringify(value);
  if (encoded === undefined) {
    return null;
  }
  return JSON.parse(encoded) as JsonValue;
}

function redactedHeaders(headers: Readonly<Record<string, string>>): Record<string, string> {
  return Object.fromEntries(
    Object.entries(headers).map(([name, value]) => [
      name,
      /authorization|cookie|credential|api[-_]?key|token/i.test(name) ? "<redacted>" : value,
    ]),
  );
}

function contentDelta(event: AssistantMessageEvent): WireStreamRecord | undefined {
  switch (event.type) {
    case "text_delta":
      return {
        event: "content_delta",
        content_index: event.contentIndex,
        delta: { type: "text", text: event.delta },
      };
    case "thinking_delta":
      return {
        event: "content_delta",
        content_index: event.contentIndex,
        delta: { type: "reasoning", text: event.delta },
      };
    case "toolcall_start": {
      const block = event.partial.content[event.contentIndex];
      if (block?.type !== "toolCall") {
        throw new Error("Pi tool-call start is missing its partial tool call");
      }
      return {
        event: "content_delta",
        content_index: event.contentIndex,
        delta: { type: "tool_call_start", id: block.id, name: block.name },
      };
    }
    case "toolcall_delta":
      return {
        event: "content_delta",
        content_index: event.contentIndex,
        delta: { type: "tool_call_arguments", json_delta: event.delta },
      };
    case "start":
    case "text_start":
    case "text_end":
    case "thinking_start":
    case "thinking_end":
    case "toolcall_end":
    case "done":
    case "error":
      return undefined;
  }
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

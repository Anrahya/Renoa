import { isContextOverflow, type AssistantMessage } from "@earendil-works/pi-ai";

import type { JsonValue, WireModelResponse } from "./contract.js";

/** Maps Pi's finished assistant message onto Renoa's wire response. */
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
          // Pi reports arguments it could not parse as `{}` beside an error.
          // A call the kernel would run with them fails the turn instead.
          if (content.argumentsError !== undefined && message.stopReason === "toolUse") {
            throw Object.assign(
              new Error(`${content.name} tool-call arguments are not valid JSON: ${content.argumentsError}`),
              { categoryHint: "protocol" },
            );
          }
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

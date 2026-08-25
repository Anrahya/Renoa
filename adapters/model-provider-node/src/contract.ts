export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

export type ProviderId = "xai" | "opencode-go";

export type ReasoningLevel =
  | "off"
  | "minimal"
  | "low"
  | "medium"
  | "high"
  | "xhigh"
  | "max";

export type FailureCategory =
  | "authentication"
  | "rate_limited"
  | "invalid_request"
  | "context_window_exceeded"
  | "network"
  | "timeout"
  | "provider_unavailable"
  | "protocol"
  | "stream_interrupted"
  | "cancelled"
  | "unknown";

export type InferenceOutcome = "known_not_started" | "unknown";

export interface WireModelRequest {
  readonly system_prompt: string;
  readonly messages: readonly WireMessage[];
  readonly tools: readonly WireTool[];
}

export type WireMessage =
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

export type WireContent =
  | { readonly type: "text"; readonly text: string }
  | { readonly type: "image"; readonly data: string; readonly mime_type: string };

export interface WireTool {
  readonly name: string;
  readonly description: string;
  readonly input_schema: JsonValue;
}

export interface WireUsage {
  readonly input: number;
  readonly output: number;
  readonly cache_read: number;
  readonly cache_write: number;
}

export interface WireMetadata {
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
  readonly usage: WireUsage;
  readonly metadata: {
    readonly api: string;
    readonly provider: string;
    readonly model: string;
    readonly response_model?: string;
    readonly response_id?: string;
    readonly raw_stop_reason?: string;
  };
}

export type WireAssistantContent =
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

export type WireStreamDelta =
  | { readonly type: "text"; readonly text: string }
  | { readonly type: "reasoning"; readonly text: string }
  | { readonly type: "tool_call_start"; readonly id: string; readonly name: string }
  | { readonly type: "tool_call_arguments"; readonly json_delta: string };

export interface WireErrorDiagnostic {
  readonly provider: string;
  readonly model: string;
  readonly http_status?: number;
  readonly provider_code?: string;
  readonly request_id?: string;
  readonly retry_after?: string;
  readonly attempt_count: number;
  readonly cause_code?: string;
  readonly cause_message?: string;
  readonly provider_message?: string;
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
      readonly event: "retry_attempt";
      readonly attempt: number;
      readonly next_attempt: number;
      readonly category: FailureCategory;
      readonly delay_ms: number;
      readonly cause_code?: string;
    }
  | {
      readonly event: "error";
      readonly error: string;
      readonly error_kind: FailureCategory;
      readonly inference_outcome: InferenceOutcome;
      readonly diagnostic: WireErrorDiagnostic;
    };

export interface CatalogModel {
  readonly id: string;
  readonly name: string;
  readonly reasoning_levels: readonly string[];
  readonly context_window_tokens: number;
  readonly model_spec: unknown;
}

export interface DescriptionResponse {
  readonly context_window_tokens: number;
  readonly max_output_tokens: number;
  readonly model_binding_id: string;
  readonly model_spec: string;
  readonly reasoning_level: ReasoningLevel;
}

export function providerDisplayName(provider: string): string {
  switch (provider) {
    case "xai":
      return "xAI";
    case "opencode-go":
      return "OpenCode Go";
    default:
      return provider;
  }
}

import { WIRE_VERSION } from "./limits.js";

export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

export type JsonObject = { [key: string]: JsonValue };

export interface FrozenMcpTool {
  readonly name: string;
  readonly input_schema: JsonObject;
  readonly output_schema?: JsonObject;
}

export interface WireAuthorization {
  readonly scheme: "bearer";
  readonly token: string;
}

export type WireHeaders = Readonly<Record<string, string>>;

export type AdapterRequest =
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly action: "discover";
      readonly endpoint: string;
      readonly headers?: WireHeaders;
      readonly authorization?: WireAuthorization;
    }
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly action: "call";
      readonly endpoint: string;
      readonly protocol_version: string;
      readonly headers?: WireHeaders;
      readonly authorization?: WireAuthorization;
      readonly tool: FrozenMcpTool;
      readonly arguments: JsonObject;
    };

export interface CatalogTool {
  readonly name: string;
  readonly description: string;
  readonly input_schema: JsonObject;
  readonly model_input_schema: JsonObject;
  readonly output_schema?: JsonObject;
}

export interface RejectedTool {
  readonly index: number;
  readonly name?: string;
  readonly reason: string;
}

export interface DiscoveredCatalog {
  readonly endpoint: string;
  readonly protocol_version: string;
  readonly adapter_revision: string;
  readonly tools: readonly CatalogTool[];
  readonly rejected_tools: readonly RejectedTool[];
}

export type WireToolContent =
  | { readonly type: "text"; readonly text: string }
  | {
      readonly type: "image";
      readonly data: string;
      readonly mime_type: string;
    };

export interface WireToolResult {
  readonly content: readonly WireToolContent[];
  readonly structured_content:
    | { readonly present: false }
    | { readonly present: true; readonly value: JsonValue };
  readonly is_error: boolean;
}

export type AdapterFailureKind =
  | "invalid_request"
  | "invalid_endpoint"
  | "incompatible_protocol"
  | "protocol"
  | "resource_limit"
  | "timeout"
  | "cancelled"
  | "unavailable"
  | "unsupported_result"
  | "invalid_result"
  | "transport"
  | "internal";

export type OutcomeCertainty = "definite" | "unknown";

export interface WireFailure {
  readonly kind: AdapterFailureKind;
  readonly certainty: OutcomeCertainty;
  readonly message: string;
  readonly partial_changes_possible: boolean;
  readonly diagnostic: {
    readonly code?: string;
    readonly http_status?: number;
    readonly detail: string;
  };
}

export type AdapterRecord =
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly event: "dispatch_started";
    }
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly event: "discovered";
      readonly catalog: DiscoveredCatalog;
    }
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly event: "completed";
      readonly result: WireToolResult;
    }
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly event: "failed";
      readonly failure: WireFailure;
    };

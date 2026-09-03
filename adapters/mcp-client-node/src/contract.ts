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

export interface WireCredential {
  readonly scheme: "header";
  readonly name: string;
  readonly prefix: string;
  readonly secret: string;
}

export interface WireOAuthAuthorization {
  readonly scheme: "bearer";
  readonly token: string;
}

export interface WireOAuthState extends JsonObject {
  readonly schema_version: 1;
  readonly mcp_endpoint: string;
  readonly csrf_state: string;
  readonly redirect_uri: string;
}

export type WireOAuthRegistration =
  | { readonly mode: "dynamic" }
  | {
      readonly mode: "client_metadata";
      readonly client_metadata_url: string;
    }
  | {
      readonly mode: "pre_registered";
      readonly issuer: string;
      readonly client_id: string;
      readonly client_secret?: string;
    };

export type WireHeaders = Readonly<Record<string, string>>;

export type AdapterRequest =
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly action: "discover";
      readonly endpoint: string;
      readonly headers?: WireHeaders;
      readonly credential?: WireCredential;
    }
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly action: "call";
      readonly endpoint: string;
      readonly protocol_version: string;
      readonly headers?: WireHeaders;
      readonly credential?: WireCredential;
      readonly tool: FrozenMcpTool;
      readonly arguments: JsonObject;
    }
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly action: "oauth_begin";
      readonly endpoint: string;
      readonly csrf_state: string;
      readonly redirect_uri: string;
      readonly force_reauthorization: boolean;
      readonly requested_scope?: string;
      readonly registration: WireOAuthRegistration;
      readonly oauth_state?: WireOAuthState;
    }
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly action: "oauth_exchange";
      readonly endpoint: string;
      readonly authorization_code: string;
      readonly issuer?: string;
      readonly registration: WireOAuthRegistration;
      readonly oauth_state: WireOAuthState;
    }
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly action: "oauth_token";
      readonly endpoint: string;
      readonly oauth_state: WireOAuthState;
    }
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly action: "oauth_refresh";
      readonly endpoint: string;
      readonly registration: WireOAuthRegistration;
      readonly oauth_state: WireOAuthState;
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
    readonly required_scope?: string;
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
    }
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly event: "oauth_redirect";
      readonly authorization_url: string;
      readonly oauth_state: WireOAuthState;
    }
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly event: "oauth_authorized";
      readonly authorization: WireOAuthAuthorization;
      readonly oauth_state: WireOAuthState;
    }
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly event: "oauth_refresh_required";
      readonly oauth_state: WireOAuthState;
    }
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly event: "oauth_failed";
      readonly failure: WireFailure;
      readonly oauth_state: WireOAuthState;
    };

export type CatalogRequest = SearchRequest | ResolveRequest;

export interface SearchRequest {
  readonly wire_version: 1;
  readonly action: "search";
  readonly query: string;
}

export interface ResolveRequest {
  readonly wire_version: 1;
  readonly action: "resolve";
  readonly candidate: string;
}

export interface CatalogCandidate {
  readonly reference: string;
  readonly name: string;
  readonly description: string;
  readonly domain: string;
  readonly server: string;
  readonly endpoint: string;
  readonly transport: "streamable-http";
  readonly docs?: string;
  readonly auth: CatalogAuth;
  readonly source: {
    readonly provider: "integrations.sh";
    readonly record: string;
    readonly evidence: readonly string[];
  };
}

export type CatalogAuth =
  | { readonly status: "none" }
  | {
      readonly status: "required" | "optional" | "unknown";
      readonly setup?: string;
      readonly blocker: string;
    };

export type AdapterRecord = CompletedRecord | FailedRecord;

export type CompletedRecord =
  | {
      readonly wire_version: 1;
      readonly event: "completed";
      readonly adapter_revision: string;
      readonly result: {
        readonly action: "search";
        readonly candidates: readonly CatalogCandidate[];
      };
    }
  | {
      readonly wire_version: 1;
      readonly event: "completed";
      readonly adapter_revision: string;
      readonly result: {
        readonly action: "resolve";
        readonly candidate: CatalogCandidate;
      };
    };

export interface FailedRecord {
  readonly wire_version: 1;
  readonly event: "failed";
  readonly failure: {
    readonly kind:
      | "invalid_request"
      | "not_found"
      | "conflict"
      | "unavailable"
      | "protocol"
      | "resource_limit"
      | "internal";
    readonly message: string;
    readonly diagnostic?: {
      readonly code?: string;
      readonly http_status?: number;
      readonly detail?: string;
    };
  };
}

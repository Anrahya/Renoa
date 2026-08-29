import type { WIRE_VERSION } from "./limits.js";

export type RegistryRequest = SearchRequest | LookupRequest;

export interface SearchRequest {
  readonly wire_version: typeof WIRE_VERSION;
  readonly action: "search";
  readonly query: string;
}

export interface LookupRequest {
  readonly wire_version: typeof WIRE_VERSION;
  readonly action: "lookup";
  readonly registry_name: string;
  readonly registry_version: string;
}

export type PublisherVerification = "domain" | "github";
export type RegistryStatus = "active" | "deprecated" | "deleted";

export interface RegistryPublisher {
  readonly namespace: string;
  readonly verification: PublisherVerification;
}

export interface RegistryRepository {
  readonly url: string;
  readonly source: string;
  readonly id?: string;
}

export interface RegistryCandidate {
  readonly registry_name: string;
  readonly registry_version: string;
  readonly title?: string;
  readonly publisher_description: string;
  readonly publisher: RegistryPublisher;
  readonly publisher_namespace_matches_query: boolean;
  readonly status: RegistryStatus;
  readonly website_url?: string;
  readonly repository?: RegistryRepository;
  readonly remote_count: number;
  readonly streamable_http_count: number;
  readonly package_count: number;
}

export interface SearchCoverage {
  readonly returned: number;
  readonly unique_seen: number;
  readonly rejected_records: number;
  readonly filtered_records: number;
  readonly source_truncated: boolean;
  readonly output_truncated: boolean;
}

export interface RegistrySearchResult {
  readonly action: "search";
  readonly source: "official_mcp_registry";
  readonly query: string;
  readonly normalized_queries: readonly string[];
  readonly candidates: readonly RegistryCandidate[];
  readonly coverage: SearchCoverage;
  readonly trust: RegistryTrust;
  readonly next_action: string;
}

export interface RegistryTrust {
  readonly verified: "publisher_namespace_control";
  readonly not_verified: readonly [
    "provider_endorsement",
    "metadata_accuracy",
    "server_safety",
    "endpoint_behavior",
  ];
}

export interface RegistryInputRequirement {
  readonly name: string;
  readonly required: boolean;
  readonly secret: boolean;
  readonly description?: string;
}

export type RemoteBlocker =
  | "unsupported_transport"
  | "sse_transport_unsupported"
  | "https_required"
  | "endpoint_template_unsupported";

export interface RegistryRemote {
  readonly transport: "streamable-http" | "sse" | "unknown";
  readonly declared_transport?: string;
  readonly url: string;
  readonly transport_supported: boolean;
  readonly blocker?: RemoteBlocker;
  readonly headers: readonly RegistryInputRequirement[];
}

export interface RegistryPackage {
  readonly registry_type: string;
  readonly identifier: string;
  readonly version?: string;
  readonly transport: "stdio" | "streamable-http" | "sse" | "unknown";
  readonly supported_by_renoa: false;
  readonly blocker: "local_package_execution_not_supported";
}

export interface RegistryRecord {
  readonly registry_name: string;
  readonly registry_version: string;
  readonly title?: string;
  readonly publisher_description: string;
  readonly publisher: RegistryPublisher;
  readonly status: RegistryStatus;
  readonly website_url?: string;
  readonly repository?: RegistryRepository;
  readonly remotes: readonly RegistryRemote[];
  readonly packages: readonly RegistryPackage[];
  readonly source_record: string;
}

export interface RegistryLookupResult {
  readonly action: "lookup";
  readonly source: "official_mcp_registry";
  readonly record: RegistryRecord;
  readonly trust: RegistryTrust;
  readonly next_action: string;
}

export type RegistryResult = RegistrySearchResult | RegistryLookupResult;

export type FailureKind =
  | "invalid_request"
  | "not_found"
  | "unavailable"
  | "protocol"
  | "resource_limit"
  | "timeout"
  | "cancelled"
  | "internal";

export interface WireFailure {
  readonly kind: FailureKind;
  readonly message: string;
  readonly diagnostic: {
    readonly code?: string;
    readonly http_status?: number;
    readonly detail: string;
  };
}

export type AdapterRecord =
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly event: "completed";
      readonly adapter_revision: string;
      readonly result: RegistryResult;
    }
  | {
      readonly wire_version: typeof WIRE_VERSION;
      readonly event: "failed";
      readonly failure: WireFailure;
    };

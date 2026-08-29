mod validate;

use serde::{Deserialize, Serialize};

use super::{RegistryError, RegistryFailure};

pub(super) const SEARCH_NEXT_ACTION: &str = "Call lookup with one exact registry_name and registry_version. Registry publication proves namespace control only; verify provider ownership, endpoint, and authentication in official provider documentation before add.";
pub(super) const EMPTY_SEARCH_NEXT_ACTION: &str = "Retry once with only the provider or product name. If the official Registry still has no usable name match, search the provider's official website; do not guess an endpoint.";
pub(super) const LOOKUP_NEXT_ACTION: &str = "Treat this as publisher metadata only. Verify the selected endpoint and authentication against the provider's official HTTPS documentation. Then call add with kind=mcp and the exact reviewed values; never copy secret header values from registry metadata.";

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum AdapterRecord {
    Completed {
        wire_version: u32,
        adapter_revision: String,
        result: Box<RegistryResult>,
    },
    Failed {
        wire_version: u32,
        failure: RegistryFailure,
    },
}

impl AdapterRecord {
    pub(super) const fn wire_version(&self) -> u32 {
        match self {
            Self::Completed { wire_version, .. } | Self::Failed { wire_version, .. } => {
                *wire_version
            }
        }
    }

    pub(super) fn into_result(
        self,
        expected_revision: &str,
    ) -> Result<RegistryResult, RegistryError> {
        match self {
            Self::Completed {
                adapter_revision,
                result,
                ..
            } => {
                if adapter_revision != expected_revision {
                    return Err(RegistryError::Protocol(format!(
                        "MCP Registry adapter returned revision '{adapter_revision}', expected '{expected_revision}'"
                    )));
                }
                result.validate()?;
                Ok(*result)
            }
            Self::Failed { failure, .. } => {
                failure.validate()?;
                Err(RegistryError::Remote(failure))
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum RegistryResult {
    Search(RegistrySearchResult),
    Lookup(Box<RegistryLookupResult>),
}

impl RegistryResult {
    fn validate(&self) -> Result<(), RegistryError> {
        match self {
            Self::Search(result) => validate::search(result),
            Self::Lookup(result) => validate::lookup(result),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegistrySearchResult {
    source: RegistrySource,
    query: String,
    normalized_queries: Vec<String>,
    candidates: Vec<RegistryCandidate>,
    coverage: SearchCoverage,
    trust: RegistryTrust,
    next_action: String,
}

impl RegistrySearchResult {
    pub(super) fn query(&self) -> &str {
        &self.query
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegistryCandidate {
    registry_name: String,
    registry_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    publisher_description: String,
    publisher: RegistryPublisher,
    publisher_namespace_matches_query: bool,
    status: RegistryStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    website_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repository: Option<RegistryRepository>,
    remote_count: usize,
    streamable_http_count: usize,
    package_count: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SearchCoverage {
    returned: usize,
    unique_seen: usize,
    rejected_records: usize,
    filtered_records: usize,
    source_truncated: bool,
    output_truncated: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegistryLookupResult {
    source: RegistrySource,
    record: RegistryServerRecord,
    trust: RegistryTrust,
    next_action: String,
}

impl RegistryLookupResult {
    pub(super) fn identity(&self) -> (&str, &str) {
        (&self.record.registry_name, &self.record.registry_version)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RegistryServerRecord {
    registry_name: String,
    registry_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    publisher_description: String,
    publisher: RegistryPublisher,
    status: RegistryStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    website_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repository: Option<RegistryRepository>,
    remotes: Vec<RegistryRemote>,
    packages: Vec<RegistryPackage>,
    source_record: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum RegistrySource {
    OfficialMcpRegistry,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RegistryPublisher {
    namespace: String,
    verification: PublisherVerification,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PublisherVerification {
    Domain,
    Github,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RegistryRepository {
    url: String,
    source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum RegistryStatus {
    Active,
    Deprecated,
    Deleted,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RegistryTrust {
    verified: VerifiedClaim,
    not_verified: [UnverifiedClaim; 4],
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum VerifiedClaim {
    PublisherNamespaceControl,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum UnverifiedClaim {
    ProviderEndorsement,
    MetadataAccuracy,
    ServerSafety,
    EndpointBehavior,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RegistryRemote {
    transport: RegistryTransport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    declared_transport: Option<String>,
    url: String,
    transport_supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    blocker: Option<RemoteBlocker>,
    headers: Vec<RegistryInputRequirement>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum RegistryTransport {
    StreamableHttp,
    Sse,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RemoteBlocker {
    UnsupportedTransport,
    SseTransportUnsupported,
    HttpsRequired,
    EndpointTemplateUnsupported,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RegistryInputRequirement {
    name: String,
    required: bool,
    secret: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RegistryPackage {
    registry_type: String,
    identifier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    transport: PackageTransport,
    supported_by_renoa: bool,
    blocker: PackageBlocker,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum PackageTransport {
    Stdio,
    StreamableHttp,
    Sse,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PackageBlocker {
    LocalPackageExecutionNotSupported,
}

mod error;
mod process;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub use error::{CatalogDiagnostic, CatalogError, CatalogFailure, CatalogFailureKind};

const WIRE_VERSION: u32 = 1;
const ADAPTER_REVISION: &str = "integration-catalog-node-v0.1.0";

#[derive(Clone)]
pub(crate) struct IntegrationCatalog {
    adapter: PathBuf,
}

impl IntegrationCatalog {
    pub(crate) fn resolve_adapter(path: &Path) -> Result<PathBuf, CatalogError> {
        let resolved = std::fs::canonicalize(path).map_err(CatalogError::Resolve)?;
        if !resolved.is_file() {
            return Err(CatalogError::NotFile(resolved));
        }
        Ok(resolved)
    }

    pub(crate) const fn new(adapter: PathBuf) -> Self {
        Self { adapter }
    }

    pub(crate) async fn search(
        &self,
        query: &str,
        cancellation: CancellationToken,
    ) -> Result<Vec<CatalogCandidate>, CatalogError> {
        let record = process::run(
            &self.adapter,
            &CatalogRequest::Search {
                wire_version: WIRE_VERSION,
                query,
            },
            cancellation,
        )
        .await?;
        match checked_result(record)? {
            CatalogResult::Search { candidates } => Ok(candidates),
            CatalogResult::Resolve { .. } => Err(CatalogError::Protocol(
                "catalog adapter returned resolve data for a search request".to_owned(),
            )),
        }
    }

    pub(crate) async fn resolve(
        &self,
        reference: &str,
        cancellation: CancellationToken,
    ) -> Result<CatalogCandidate, CatalogError> {
        let record = process::run(
            &self.adapter,
            &CatalogRequest::Resolve {
                wire_version: WIRE_VERSION,
                candidate: reference,
            },
            cancellation,
        )
        .await?;
        match checked_result(record)? {
            CatalogResult::Resolve { candidate } => Ok(*candidate),
            CatalogResult::Search { .. } => Err(CatalogError::Protocol(
                "catalog adapter returned search data for a resolve request".to_owned(),
            )),
        }
    }
}

fn checked_result(record: CatalogRecord) -> Result<CatalogResult, CatalogError> {
    match record {
        CatalogRecord::Completed {
            adapter_revision,
            result,
            ..
        } => {
            if adapter_revision != ADAPTER_REVISION {
                return Err(CatalogError::Protocol(format!(
                    "catalog adapter returned revision '{adapter_revision}', expected '{ADAPTER_REVISION}'"
                )));
            }
            validate_result(&result)?;
            Ok(result)
        }
        CatalogRecord::Failed { failure, .. } => {
            failure.validate()?;
            Err(CatalogError::Remote(failure))
        }
    }
}

fn validate_result(result: &CatalogResult) -> Result<(), CatalogError> {
    match result {
        CatalogResult::Search { candidates } => {
            if candidates.len() > 10 {
                return Err(CatalogError::Protocol(
                    "catalog adapter returned more than 10 candidates".to_owned(),
                ));
            }
            let mut references = HashSet::new();
            for candidate in candidates {
                candidate.validate()?;
                if !references.insert(candidate.reference()) {
                    return Err(CatalogError::Protocol(
                        "catalog adapter repeated a candidate reference".to_owned(),
                    ));
                }
            }
            Ok(())
        }
        CatalogResult::Resolve { candidate } => candidate.validate(),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogCandidate {
    reference: String,
    name: String,
    description: String,
    domain: String,
    server: String,
    endpoint: String,
    transport: CatalogTransport,
    #[serde(skip_serializing_if = "Option::is_none")]
    docs: Option<String>,
    auth: CatalogAuth,
    source: CatalogSource,
}

impl CatalogCandidate {
    pub(crate) fn reference(&self) -> &str {
        &self.reference
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn description(&self) -> &str {
        &self.description
    }

    pub(crate) fn server(&self) -> &str {
        &self.server
    }

    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub(crate) fn docs(&self) -> Option<&str> {
        self.docs.as_deref()
    }

    pub(crate) fn source_record(&self) -> &str {
        &self.source.record
    }

    fn validate(&self) -> Result<(), CatalogError> {
        require_bounded("candidate name", &self.name, 1, 256)?;
        require_bounded("candidate description", &self.description, 0, 8 * 1_024)?;
        require_ascii_identity("candidate domain", &self.domain, 253, b".-")?;
        if !self.domain.contains('.') {
            return Err(protocol("candidate domain must contain a dot"));
        }
        require_ascii_identity("candidate server", &self.server, 256, b"_-.")?;
        require_https_url("candidate endpoint", &self.endpoint, 8 * 1_024)?;
        if let Some(docs) = &self.docs {
            require_https_url("candidate docs", docs, 8 * 1_024)?;
        }
        require_https_url("candidate source record", &self.source.record, 8 * 1_024)?;
        if self.source.record != format!("https://integrations.sh/{}/", self.domain) {
            return Err(protocol(
                "candidate source record does not match its domain",
            ));
        }
        if self.source.evidence.len() > 32 {
            return Err(protocol("candidate has more than 32 evidence URLs"));
        }
        for evidence in &self.source.evidence {
            require_https_url("candidate evidence", evidence, 8 * 1_024)?;
        }
        match &self.auth {
            CatalogAuth::None => {}
            CatalogAuth::Required { setup, blocker }
            | CatalogAuth::Optional { setup, blocker }
            | CatalogAuth::Unknown { setup, blocker } => {
                require_bounded("candidate auth blocker", blocker, 1, 8 * 1_024)?;
                if let Some(setup) = setup {
                    require_bounded("candidate auth setup", setup, 1, 8 * 1_024)?;
                }
            }
        }
        let parts = self.reference.split('/').collect::<Vec<_>>();
        if parts.len() != 4
            || parts[0] != "integrations.sh"
            || parts[1] != self.domain
            || parts[2] != self.server
            || parts[3].len() != 64
            || !parts[3]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(protocol("candidate reference is malformed or mismatched"));
        }
        Ok(())
    }
}

fn require_bounded(
    field: &str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), CatalogError> {
    if (minimum..=maximum).contains(&value.len()) {
        Ok(())
    } else {
        Err(protocol(format!(
            "{field} must contain {minimum}-{maximum} UTF-8 bytes"
        )))
    }
}

fn require_ascii_identity(
    field: &str,
    value: &str,
    maximum: usize,
    punctuation: &[u8],
) -> Result<(), CatalogError> {
    if !value.is_empty()
        && value.len() <= maximum
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || punctuation.contains(&byte)
        })
    {
        Ok(())
    } else {
        Err(protocol(format!("{field} is malformed")))
    }
}

fn require_https_url(field: &str, value: &str, maximum: usize) -> Result<(), CatalogError> {
    require_bounded(field, value, 1, maximum)?;
    let url =
        url::Url::parse(value).map_err(|error| protocol(format!("{field} is invalid: {error}")))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host().is_none()
    {
        return Err(protocol(format!(
            "{field} must be HTTPS without credentials or a fragment"
        )));
    }
    Ok(())
}

fn protocol(message: impl Into<String>) -> CatalogError {
    CatalogError::Protocol(message.into())
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum CatalogTransport {
    StreamableHttp,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum CatalogAuth {
    None,
    Required {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        setup: Option<String>,
        blocker: String,
    },
    Optional {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        setup: Option<String>,
        blocker: String,
    },
    Unknown {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        setup: Option<String>,
        blocker: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CatalogSource {
    provider: CatalogProvider,
    record: String,
    evidence: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum CatalogProvider {
    #[serde(rename = "integrations.sh")]
    IntegrationsSh,
}

#[derive(Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum CatalogRequest<'a> {
    Search {
        wire_version: u32,
        query: &'a str,
    },
    Resolve {
        wire_version: u32,
        candidate: &'a str,
    },
}

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum CatalogRecord {
    Completed {
        wire_version: u32,
        adapter_revision: String,
        result: CatalogResult,
    },
    Failed {
        wire_version: u32,
        failure: CatalogFailure,
    },
}

impl CatalogRecord {
    fn wire_version(&self) -> u32 {
        match self {
            Self::Completed { wire_version, .. } | Self::Failed { wire_version, .. } => {
                *wire_version
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum CatalogResult {
    Search { candidates: Vec<CatalogCandidate> },
    Resolve { candidate: Box<CatalogCandidate> },
}

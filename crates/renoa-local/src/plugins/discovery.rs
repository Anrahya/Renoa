mod contract;
mod error;
mod process;

use std::path::{Path, PathBuf};

use serde::Serialize;
use tokio_util::sync::CancellationToken;

pub(crate) use contract::{RegistryLookupResult, RegistrySearchResult};
pub(crate) use error::{RegistryError, RegistryFailure, RegistryFailureKind};

use contract::RegistryResult;

const WIRE_VERSION: u32 = 1;
const ADAPTER_REVISION: &str = "mcp-registry-node-v0.1.0";

#[derive(Clone)]
pub(crate) struct OfficialRegistry {
    adapter: PathBuf,
}

impl OfficialRegistry {
    pub(crate) fn resolve_adapter(path: &Path) -> Result<PathBuf, RegistryError> {
        let resolved = std::fs::canonicalize(path).map_err(RegistryError::Resolve)?;
        if !resolved.is_file() {
            return Err(RegistryError::NotFile(resolved));
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
    ) -> Result<RegistrySearchResult, RegistryError> {
        let query = query.trim();
        validate_query(query)?;
        let record = process::run(
            &self.adapter,
            &RegistryRequest::Search {
                wire_version: WIRE_VERSION,
                query,
            },
            cancellation,
        )
        .await?;
        match record.into_result(ADAPTER_REVISION)? {
            RegistryResult::Search(result) if result.query() == query => Ok(result),
            RegistryResult::Search(_) => Err(RegistryError::Protocol(
                "MCP Registry adapter changed the search query".to_owned(),
            )),
            RegistryResult::Lookup(_) => Err(RegistryError::Protocol(
                "MCP Registry adapter returned lookup data for a search request".to_owned(),
            )),
        }
    }

    pub(crate) async fn lookup(
        &self,
        registry_name: &str,
        registry_version: &str,
        cancellation: CancellationToken,
    ) -> Result<RegistryLookupResult, RegistryError> {
        validate_exact_identity(registry_name, registry_version)?;
        let record = process::run(
            &self.adapter,
            &RegistryRequest::Lookup {
                wire_version: WIRE_VERSION,
                registry_name,
                registry_version,
            },
            cancellation,
        )
        .await?;
        match record.into_result(ADAPTER_REVISION)? {
            RegistryResult::Lookup(result)
                if result.identity() == (registry_name, registry_version) =>
            {
                Ok(*result)
            }
            RegistryResult::Lookup(_) => Err(RegistryError::Protocol(
                "MCP Registry adapter changed the exact lookup identity".to_owned(),
            )),
            RegistryResult::Search(_) => Err(RegistryError::Protocol(
                "MCP Registry adapter returned search data for a lookup request".to_owned(),
            )),
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum RegistryRequest<'a> {
    Search {
        wire_version: u32,
        query: &'a str,
    },
    Lookup {
        wire_version: u32,
        registry_name: &'a str,
        registry_version: &'a str,
    },
}

fn validate_query(query: &str) -> Result<(), RegistryError> {
    if query.trim().is_empty() || query.len() > 256 || query.chars().any(char::is_control) {
        Err(RegistryError::Invalid(
            "search query must contain 1-256 UTF-8 bytes".to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn validate_exact_identity(name: &str, version: &str) -> Result<(), RegistryError> {
    let Some((namespace, leaf)) = name.split_once('/') else {
        return Err(RegistryError::Invalid(
            "registry_name is not a valid MCP Registry server name".to_owned(),
        ));
    };
    if name.len() > 200
        || namespace.is_empty()
        || leaf.is_empty()
        || name.matches('/').count() != 1
        || !namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-".contains(&byte))
        || !leaf
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err(RegistryError::Invalid(
            "registry_name is not a valid MCP Registry server name".to_owned(),
        ));
    }
    if version.is_empty()
        || version.len() > 255
        || version == "latest"
        || version
            .bytes()
            .any(|byte| byte == b'/' || byte.is_ascii_control())
    {
        return Err(RegistryError::Invalid(
            "registry_version must be an exact 1-255 byte version, not 'latest'".to_owned(),
        ));
    }
    Ok(())
}

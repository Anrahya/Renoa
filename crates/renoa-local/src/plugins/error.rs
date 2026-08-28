use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PluginError {
    #[error("invalid Agent Plugin: {0}")]
    Invalid(String),
    #[error("Agent Plugin conflicts with durable state: {0}")]
    Conflict(String),
    #[error("Agent Plugin is unavailable: {0}")]
    NotFound(String),
    #[error("Agent Plugin operation is unavailable: {0}")]
    Unavailable(String),
    #[error("Agent Plugin storage failed while {action} `{path}`: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Agent Plugin Host catalog failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("Agent Plugin JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    HostCatalog(#[from] crate::host::catalog::HostCatalogError),
    #[error(transparent)]
    Mcp(#[from] crate::mcp::McpHostError),
    #[error(transparent)]
    Skill(#[from] crate::skills::SkillError),
    #[error(transparent)]
    Catalog(#[from] super::catalog::CatalogError),
    #[error("Agent Plugin background task failed: {0}")]
    Background(#[from] tokio::task::JoinError),
}

impl PluginError {
    pub(crate) fn from_tree(error: crate::package_tree::TreeError) -> Self {
        match error {
            crate::package_tree::TreeError::Invalid(message) => Self::Invalid(message),
            crate::package_tree::TreeError::Conflict(message) => Self::Conflict(message),
            crate::package_tree::TreeError::Io {
                action,
                path,
                source,
            } => Self::Io {
                action,
                path,
                source,
            },
        }
    }
}

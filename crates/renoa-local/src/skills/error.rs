use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SkillError {
    #[error("invalid skill configuration: {0}")]
    Invalid(String),
    #[error("skill configuration conflicts with durable state: {0}")]
    Conflict(String),
    #[error("skill is unavailable: {0}")]
    NotFound(String),
    #[error("skill storage failed while {action} `{path}`: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("skill Host catalog failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    HostCatalog(#[from] crate::host::catalog::HostCatalogError),
}

impl SkillError {
    pub(crate) fn io(
        action: &'static str,
        path: impl Into<PathBuf>,
        source: std::io::Error,
    ) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}

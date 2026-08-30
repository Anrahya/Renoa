mod archive;
mod client;
mod state;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use renoa_registry_protocol::{RegistryChanges, RegistryId, Sha256Digest};
use serde::Serialize;
use thiserror::Error;
use tokio::sync::Mutex;

use self::{archive::PackageArchive, client::RegistryClient, state::RegistryState};
use crate::plugins::{PluginError, store::PluginStore};

const TRANSFER_DIRECTORY: &str = "shared-registry";

#[derive(Clone)]
pub(crate) struct SharedPluginRegistry {
    client: RegistryClient,
    state: RegistryState,
    transfer: Arc<PathBuf>,
    synchronization: Arc<Mutex<()>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SharedPluginSyncReport {
    published: usize,
    downloaded: usize,
    revision: u64,
}

impl SharedPluginSyncReport {
    #[must_use]
    pub const fn published(self) -> usize {
        self.published
    }

    #[must_use]
    pub const fn downloaded(self) -> usize {
        self.downloaded
    }

    #[must_use]
    pub const fn revision(self) -> u64 {
        self.revision
    }
}

#[derive(Debug, Error)]
pub(crate) enum SharedRegistryError {
    #[error("invalid shared registry configuration: {0}")]
    Configuration(String),
    #[error("shared registry transport failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("shared registry returned HTTP {status} ({code}): {message}")]
    Server {
        status: u16,
        code: String,
        message: String,
    },
    #[error("shared registry protocol failed: {0}")]
    Protocol(String),
    #[error("shared registry content conflicts with durable state: {0}")]
    Conflict(String),
    #[error("shared registry package archive is invalid: {0}")]
    Archive(String),
    #[error("shared registry I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("shared registry JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    HostCatalog(#[from] crate::host::catalog::HostCatalogError),
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Plugin(#[from] PluginError),
    #[error("shared registry background task failed: {0}")]
    Background(#[from] tokio::task::JoinError),
}

impl SharedRegistryError {
    pub(crate) fn into_plugin(self) -> PluginError {
        match self {
            Self::Plugin(error) => error,
            Self::Conflict(message) | Self::Archive(message) | Self::Protocol(message) => {
                PluginError::Conflict(format!("shared registry synchronization failed: {message}"))
            }
            error => PluginError::Unavailable(error.to_string()),
        }
    }
}

impl SharedPluginRegistry {
    pub(crate) fn new(
        endpoint: &str,
        database: PathBuf,
        data_directory: &Path,
    ) -> Result<Self, SharedRegistryError> {
        let transfer = data_directory.join(TRANSFER_DIRECTORY);
        fs::create_dir_all(&transfer)?;
        owner_only_directory(&transfer)?;
        Ok(Self {
            client: RegistryClient::new(endpoint)?,
            state: RegistryState::new(database),
            transfer: Arc::new(transfer),
            synchronization: Arc::new(Mutex::new(())),
        })
    }

    pub(crate) async fn synchronize(
        &self,
        store: &PluginStore,
    ) -> Result<SharedPluginSyncReport, SharedRegistryError> {
        let _guard = self.synchronization.lock().await;
        let status = self.client.status().await?;
        let registry_id = status.registry_id();
        let mut cursor = blocking({
            let state = self.state.clone();
            move || state.bind(registry_id)
        })
        .await?;
        if status.current_revision() < cursor.revision {
            return Err(SharedRegistryError::Conflict(format!(
                "shared registry revision {} is behind this Host at {}",
                status.current_revision(),
                cursor.revision
            )));
        }
        let mut report = SharedPluginSyncReport::default();
        let installed = blocking({
            let store = store.clone();
            move || store.list().map_err(SharedRegistryError::from)
        })
        .await?;
        for plugin in installed {
            let package = Sha256Digest::parse(plugin.digest().to_owned())
                .map_err(|error| SharedRegistryError::Protocol(error.to_string()))?;
            if !self.client.contains(&package).await? {
                let archive = blocking({
                    let store = store.clone();
                    let transfer = Arc::clone(&self.transfer);
                    let digest = plugin.digest().to_owned();
                    move || PackageArchive::build(&store, &digest, &transfer)
                })
                .await?;
                let published = self.client.publish(&package, &archive).await?;
                require_registry(registry_id, published.registry_id())?;
                if published.disposition() == renoa_registry_protocol::PublishDisposition::Published
                {
                    report.published += 1;
                }
            }
        }
        loop {
            let changes = self.client.changes(cursor.revision).await?;
            require_registry(registry_id, changes.registry_id())?;
            validate_changes(cursor.revision, &changes)?;
            if changes.packages().is_empty() {
                if changes.current_revision() != cursor.revision {
                    return Err(SharedRegistryError::Protocol(format!(
                        "shared registry reported revision {} without returning its next record",
                        changes.current_revision()
                    )));
                }
                report.revision = cursor.revision;
                return Ok(report);
            }
            for record in changes.packages() {
                let digest = record.package_digest().as_str().to_owned();
                let available = blocking({
                    let store = store.clone();
                    let digest = digest.clone();
                    move || match store.load(&digest) {
                        Ok(_) => Ok(true),
                        Err(PluginError::NotFound(_)) => Ok(false),
                        Err(error) => Err(SharedRegistryError::from(error)),
                    }
                })
                .await?;
                if !available {
                    let archive = self.client.download(record, &self.transfer).await?;
                    blocking({
                        let store = store.clone();
                        let transfer = Arc::clone(&self.transfer);
                        let package = record.package_digest().clone();
                        move || archive::install_archive(&store, &archive, &package, &transfer)
                    })
                    .await?;
                    report.downloaded += 1;
                }
                cursor = blocking({
                    let state = self.state.clone();
                    let registry_id = changes.registry_id();
                    let revision = record.revision();
                    move || state.advance(registry_id, revision)
                })
                .await?;
            }
            if cursor.revision >= changes.current_revision() {
                report.revision = cursor.revision;
                return Ok(report);
            }
        }
    }
}

fn validate_changes(after: u64, changes: &RegistryChanges) -> Result<(), SharedRegistryError> {
    if changes.current_revision() < after {
        return Err(SharedRegistryError::Protocol(format!(
            "shared registry moved backward from {after} to {}",
            changes.current_revision()
        )));
    }
    let mut expected = after;
    for record in changes.packages() {
        expected = expected.checked_add(1).ok_or_else(|| {
            SharedRegistryError::Protocol("shared registry revision overflowed".to_owned())
        })?;
        if record.revision() != expected || record.revision() > changes.current_revision() {
            return Err(SharedRegistryError::Protocol(
                "shared registry returned a non-contiguous change page".to_owned(),
            ));
        }
    }
    Ok(())
}

fn require_registry(expected: RegistryId, observed: RegistryId) -> Result<(), SharedRegistryError> {
    if expected == observed {
        Ok(())
    } else {
        Err(SharedRegistryError::Conflict(format!(
            "shared registry identity changed from {expected} to {observed}"
        )))
    }
}

async fn blocking<T, F>(operation: F) -> Result<T, SharedRegistryError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, SharedRegistryError> + Send + 'static,
{
    tokio::task::spawn_blocking(operation).await?
}

#[cfg(unix)]
fn owner_only_directory(path: &Path) -> Result<(), SharedRegistryError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn owner_only_directory(_path: &Path) -> Result<(), SharedRegistryError> {
    Ok(())
}

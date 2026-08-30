use std::path::PathBuf;

use super::{super::PluginListReport, PluginManager};
use crate::{
    InstalledPlugin, PluginError, PluginInspection,
    shared_registry::{SharedPluginSyncReport, SharedRegistryError},
};

impl PluginManager {
    pub(crate) async fn inspect(
        &self,
        source: impl Into<PathBuf>,
    ) -> Result<PluginInspection, PluginError> {
        let source = source.into();
        tokio::task::spawn_blocking(move || {
            super::super::inspect::inspect(&source).map(|item| item.inspection)
        })
        .await?
    }

    pub(crate) async fn install(
        &self,
        source: impl Into<PathBuf>,
        expected_digest: impl Into<String>,
    ) -> Result<InstalledPlugin, PluginError> {
        let source = source.into();
        let expected_digest = expected_digest.into();
        let store = self.store.clone();
        let installed =
            tokio::task::spawn_blocking(move || store.install(&source, &expected_digest)).await??;
        self.synchronize_installed(&installed).await?;
        Ok(installed)
    }

    pub(crate) async fn list(&self) -> Result<Vec<InstalledPlugin>, PluginError> {
        self.synchronize_shared().await?;
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.list()).await?
    }

    pub(crate) async fn list_report(&self) -> Result<PluginListReport, PluginError> {
        self.synchronize_shared().await?;
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.list_report()).await?
    }

    pub(super) async fn synchronize_shared(&self) -> Result<SharedPluginSyncReport, PluginError> {
        let Some(registry) = &self.shared_registry else {
            return Ok(SharedPluginSyncReport::default());
        };
        registry
            .synchronize(&self.store)
            .await
            .map_err(SharedRegistryError::into_plugin)
    }

    pub(super) async fn synchronize_installed(
        &self,
        installed: &InstalledPlugin,
    ) -> Result<(), PluginError> {
        self.synchronize_shared().await.map(|_| ()).map_err(|error| {
            PluginError::Unavailable(format!(
                "package '{}' is installed locally, but shared-registry reconciliation did not finish: {error}; retrying the same install is safe",
                installed.digest()
            ))
        })
    }

    pub(crate) async fn synchronize_shared_required(
        &self,
    ) -> Result<SharedPluginSyncReport, PluginError> {
        if self.shared_registry.is_none() {
            return Err(PluginError::Unavailable(
                "RENOA_SHARED_PLUGIN_REGISTRY must be set before synchronizing the shared plugin library"
                    .to_owned(),
            ));
        }
        self.synchronize_shared().await
    }

    pub(super) async fn load_available(
        &self,
        package_digest: &str,
    ) -> Result<InstalledPlugin, PluginError> {
        let store = self.store.clone();
        let digest = package_digest.to_owned();
        match tokio::task::spawn_blocking(move || store.load(&digest)).await? {
            Ok(plugin) => Ok(plugin),
            Err(PluginError::NotFound(_)) if self.shared_registry.is_some() => {
                self.synchronize_shared().await?;
                let store = self.store.clone();
                let digest = package_digest.to_owned();
                tokio::task::spawn_blocking(move || store.load(&digest)).await?
            }
            Err(error) => Err(error),
        }
    }
}

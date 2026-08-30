use std::path::Path;

use tokio_util::sync::CancellationToken;

use crate::{
    AgentProfileId, InstalledPlugin, McpCatalogSnapshot, PluginCredential, PluginInspection,
    shared_registry::SharedPluginSyncReport,
};

use super::{LocalHost, LocalHostError};

impl LocalHost {
    /// Reconciles this Host's immutable Agent Plugin library with its configured shared registry.
    ///
    /// Existing MCP credentials, profile attachments, and sessions remain local.
    ///
    /// # Errors
    ///
    /// Returns when the registry is unreachable, changes identity, violates its
    /// ordered contract, or serves a package that fails local verification.
    pub async fn synchronize_shared_plugins(
        &self,
    ) -> Result<SharedPluginSyncReport, LocalHostError> {
        Ok(self.config.plugins.synchronize_shared_required().await?)
    }

    /// Inspects an Agent Plugins 1.0 directory without installing or executing it.
    ///
    /// # Errors
    ///
    /// Returns when the package tree or supported manifest data is invalid.
    pub async fn inspect_plugin(&self, source: &Path) -> Result<PluginInspection, LocalHostError> {
        Ok(self.config.plugins.inspect(source).await?)
    }

    /// Installs the exact package revision returned by [`Self::inspect_plugin`].
    ///
    /// # Errors
    ///
    /// Returns when the source changed, immutable storage fails, or durable
    /// metadata conflicts with the package.
    pub async fn install_plugin(
        &self,
        source: &Path,
        expected_digest: &str,
    ) -> Result<InstalledPlugin, LocalHostError> {
        Ok(self.config.plugins.install(source, expected_digest).await?)
    }

    /// Lists installed package revisions after verifying their immutable content.
    ///
    /// # Errors
    ///
    /// Returns when durable metadata or installed content is missing or corrupt.
    pub async fn installed_plugins(&self) -> Result<Vec<InstalledPlugin>, LocalHostError> {
        Ok(self.config.plugins.list().await?)
    }

    /// Connects one installed package MCP server for an exact registered profile.
    ///
    /// # Errors
    ///
    /// Returns package, credential, adapter, discovery, or durable storage failures.
    pub async fn connect_profile_plugin_mcp(
        &self,
        profile_id: &AgentProfileId,
        package_digest: &str,
        server_id: &str,
        connection_id: &str,
        credential: PluginCredential,
    ) -> Result<McpCatalogSnapshot, LocalHostError> {
        self.profile(profile_id)?;
        Ok(self
            .config
            .plugins
            .connect_profile(
                profile_id,
                package_digest,
                server_id,
                connection_id,
                credential,
                CancellationToken::new(),
            )
            .await?)
    }
}

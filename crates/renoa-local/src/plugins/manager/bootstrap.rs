use super::{PluginManager, identity::integration_id};
use crate::{
    mcp::{McpConnectionAuth, McpConnectionCandidate, McpRequestHeaders},
    plugins::{PluginError, generated::GeneratedMcpPlugin, inspect},
};

impl PluginManager {
    /// Host provisioning admits the same portable revision as agent intake. It
    /// registers configuration only; catalog discovery and agent grants are separate.
    pub(crate) async fn register_host_mcp(
        &self,
        plugin_name: &str,
        connection: &str,
        endpoint: &str,
        auth: McpConnectionAuth,
    ) -> Result<(), PluginError> {
        crate::mcp::validate_identity("connection", connection)?;
        crate::mcp::validate_endpoint(endpoint)?;
        let staging = tempfile::tempdir().map_err(|source| PluginError::Io {
            action: "create Host MCP intake staging",
            path: std::env::temp_dir(),
            source,
        })?;
        GeneratedMcpPlugin::from_host(plugin_name, endpoint).write(staging.path())?;
        let captured = inspect::inspect(staging.path())?;
        let server = captured.inspection.mcp_servers().first().ok_or_else(|| {
            PluginError::Invalid("Host MCP configuration has no supported server".to_owned())
        })?;
        let integration = integration_id(captured.inspection.digest(), server.id());
        let candidate = McpConnectionCandidate::new(
            integration.clone(),
            connection.to_owned(),
            server.endpoint().to_owned(),
            McpRequestHeaders::default(),
            auth,
        )?;
        let catalog = self.mcp_catalog.clone();
        let store = self.store.clone();
        let installed = tokio::task::spawn_blocking(move || {
            catalog.preflight_connection(&candidate, false)?;
            let installed = store.install_captured(&captured)?;
            catalog.register_connection(&integration,candidate.connection_id(),candidate.endpoint(),candidate.request_headers(),candidate.auth()).map_err(|error| PluginError::Unavailable(format!("package '{}' is installed; Host connection registration did not finish: {error}; retry the same registration", installed.digest())))?;
            Ok::<_, PluginError>(installed)
        }).await??;
        self.synchronize_installed(&installed).await
    }
}

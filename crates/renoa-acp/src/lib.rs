//! ACP v1 surface adapter for the local Renoa Host.

mod config;
mod error;
mod events;
mod prompt;
mod server;

pub use config::{
    Config, GitHubMcpInstallation, ModelCatalog, configured_model_catalog, install_github_mcp,
    synchronize_shared_plugins,
};
pub use error::ServerError;

/// Serves stable ACP v1 as newline-delimited JSON-RPC over standard I/O.
///
/// # Errors
///
/// Returns an error when configuration or the ACP transport fails.
pub async fn serve_stdio(config: Config) -> Result<(), ServerError> {
    server::serve_stdio(config).await
}

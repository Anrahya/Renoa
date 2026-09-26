use std::path::Path;

use renoa_agent::ToolUpdates;
use renoa_kernel::AgentId;
use tokio_util::sync::CancellationToken;

use super::{PluginRequest, PluginSource, inventory::PluginInventoryPage};
use crate::mcp::McpCatalogSnapshot;
use crate::plugins::{
    InstalledPlugin, PluginAddOutcome, PluginConnectionRequest, PluginCredential, PluginError,
    PluginInspection, PluginManager, intake,
    manager::{ProfileAuthorizationRequest, ProfileConnectionRequest},
};

/// A caller-owned stable operation identity and its cancellable progress channel.
/// Reuse `operation_id` when resuming the same interrupted authorization operation.
pub struct PluginInvocation<'a> {
    pub operation_id: &'a str,
    pub updates: Option<&'a ToolUpdates>,
    pub cancellation: CancellationToken,
}

/// Typed lifecycle outcomes shared by Host controls and the agent tool adapter.
pub enum PluginOutcome {
    Inspected(PluginInspection),
    Installed(InstalledPlugin),
    Added(Box<PluginAddOutcome>),
    Listed(PluginInventoryPage),
    Connected {
        package_digest: String,
        server: String,
        connection: String,
        snapshot: McpCatalogSnapshot,
    },
    Authorized {
        connection: String,
        snapshot: McpCatalogSnapshot,
    },
    Disconnected {
        connection: String,
        catalog_retained: bool,
    },
    Enabled {
        connection: String,
    },
}

impl PluginManager {
    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive dispatch table owns the lifecycle actions"
    )]
    pub(crate) async fn invoke(
        &self,
        agent_id: &AgentId,
        workspace: &Path,
        request: PluginRequest,
        invocation: PluginInvocation<'_>,
    ) -> Result<PluginOutcome, PluginError> {
        intake::require_active(&invocation.cancellation)?;
        if invocation.operation_id.is_empty()
            || invocation.operation_id.len() > 256
            || invocation.operation_id.chars().any(char::is_control)
        {
            return Err(PluginError::Invalid(
                "operation_id must be a nonempty bounded stable identity".to_owned(),
            ));
        }
        match request {
            PluginRequest::Inspect { source } => {
                intake::validate(&source)?;
                let inspected = match source {
                    PluginSource::Installed { package_digest } => {
                        let store = self.store.clone();
                        let installed =
                            tokio::task::spawn_blocking(move || store.load(&package_digest))
                                .await??;
                        super::super::PluginInspection {
                            digest: installed.digest,
                            metadata: installed.metadata,
                            mcp_servers: installed.mcp_servers,
                            notices: installed.notices,
                        }
                    }
                    source => {
                        self.capture_source(source, workspace, invocation.cancellation)
                            .await?
                            .inspection
                    }
                };
                Ok(PluginOutcome::Inspected(inspected))
            }
            PluginRequest::Install {
                source,
                expected_digest,
            } => {
                let installed = self
                    .install_source(source, workspace, &expected_digest, invocation.cancellation)
                    .await?;
                Ok(PluginOutcome::Installed(installed))
            }
            PluginRequest::Add {
                source,
                expected_digest,
                server,
                connection,
                credential,
                replace,
            } => {
                intake::validate(&source)?;
                let connection = if server.is_some()
                    || connection.is_some()
                    || credential.is_some()
                    || replace
                {
                    Some(PluginConnectionRequest::new(
                        connection,
                        server,
                        credential.map_or(PluginCredential::None, Into::into),
                        replace,
                    ))
                } else {
                    None
                };
                Ok(PluginOutcome::Added(Box::new(
                    self.add_to_profile(
                        crate::plugins::manager::AddOperationContext {
                            agent_id,
                            workspace,
                            operation_id: invocation.operation_id,
                            updates: invocation.updates,
                        },
                        source,
                        expected_digest,
                        connection,
                        invocation.cancellation,
                    )
                    .await?,
                )))
            }
            PluginRequest::List { cursor, limit } => {
                if !(1..=super::MAX_PLUGIN_PAGE).contains(&limit) {
                    return Err(PluginError::Invalid(format!(
                        "list limit must be between 1 and {}",
                        super::MAX_PLUGIN_PAGE
                    )));
                }
                let packages = self.list_report().await?;
                let connections = self.connection_statuses(agent_id).await?;
                let skills = self.skill_source_reports(agent_id).await?;
                Ok(PluginOutcome::Listed(PluginInventoryPage::new(
                    &packages,
                    &connections,
                    &skills,
                    cursor.as_deref(),
                    limit,
                )?))
            }
            PluginRequest::Connect {
                package_digest,
                server,
                connection,
                credential,
                replace,
                restart,
                required_scope,
            } => {
                let snapshot = self
                    .connect_profile_operation(
                        ProfileConnectionRequest {
                            agent_id,
                            package_digest: &package_digest,
                            server_id: &server,
                            connection_id: &connection,
                            credential: credential.map_or(PluginCredential::None, Into::into),
                            replace,
                            restart,
                            requested_scope: required_scope.as_deref(),
                            operation_id: invocation.operation_id,
                            updates: invocation.updates,
                        },
                        invocation.cancellation,
                    )
                    .await?;
                Ok(PluginOutcome::Connected {
                    package_digest,
                    server,
                    connection,
                    snapshot,
                })
            }
            PluginRequest::Authorize {
                connection,
                restart,
                required_scope,
            } => {
                let snapshot = self
                    .authorize_profile(
                        ProfileAuthorizationRequest {
                            agent_id,
                            connection_id: &connection,
                            operation_id: invocation.operation_id,
                            restart,
                            requested_scope: required_scope.as_deref(),
                            updates: invocation.updates,
                        },
                        invocation.cancellation,
                    )
                    .await?;
                Ok(PluginOutcome::Authorized {
                    connection,
                    snapshot,
                })
            }
            PluginRequest::Disconnect { connection } => {
                let catalog_retained = self.disconnect_agent(agent_id, connection.clone()).await?;
                Ok(PluginOutcome::Disconnected {
                    connection,
                    catalog_retained,
                })
            }
            PluginRequest::Enable { connection } => {
                self.enable_agent(agent_id, connection.clone()).await?;
                Ok(PluginOutcome::Enabled { connection })
            }
        }
    }

    pub(crate) async fn install_source(
        &self,
        source: PluginSource,
        workspace: &Path,
        expected_digest: &str,
        cancellation: CancellationToken,
    ) -> Result<InstalledPlugin, PluginError> {
        intake::validate(&source)?;
        super::super::store::validate_digest(expected_digest)?;
        if let PluginSource::Installed { package_digest } = &source {
            if package_digest != expected_digest {
                return Err(PluginError::Conflict(
                    "installed source and expected_digest differ".to_owned(),
                ));
            }
            let installed = self.load_available(package_digest).await?;
            self.synchronize_installed(&installed).await?;
            return Ok(installed);
        }
        // A replay verifies and reuses the published content even if its old
        // source disappeared. Source syntax is still validated before adoption.
        match self.load_local(expected_digest).await {
            Ok(installed) => {
                self.synchronize_installed(&installed).await?;
                return Ok(installed);
            }
            Err(PluginError::NotFound(_)) => (),
            Err(error) => return Err(error),
        }
        let captured = self
            .capture_source(source, workspace, cancellation.clone())
            .await?;
        if captured.inspection.digest != expected_digest {
            return Err(PluginError::Conflict(format!(
                "plugin source changed after inspection: expected {expected_digest}, found {}",
                captured.inspection.digest
            )));
        }
        intake::require_active(&cancellation)?;
        let store = self.store.clone();
        let installed =
            tokio::task::spawn_blocking(move || store.install_captured(&captured)).await??;
        self.synchronize_installed(&installed).await?;
        Ok(installed)
    }
}

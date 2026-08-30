use std::collections::HashMap;

use rusqlite::{OptionalExtension as _, Transaction, params};
use serde::Serialize;

use super::{McpCatalogStore, load_catalog};
use crate::mcp::{
    McpConnectionAuth, McpHostError, ResolvedMcpTool,
    registry::{McpToolReference, McpToolSummary},
    validate_identity,
};

impl McpCatalogStore {
    pub(crate) fn enable_profile_connection(
        &self,
        profile_id: &str,
        connection_id: &str,
    ) -> Result<(), McpHostError> {
        validate_identity("profile", profile_id)?;
        validate_identity("connection", connection_id)?;
        let mut connection = self.connection()?;
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let catalog_exists = transaction
            .query_row(
                "SELECT 1 FROM mcp_catalogs WHERE connection_id = ?1",
                [connection_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !catalog_exists {
            return Err(McpHostError::NotFound(format!(
                "connection '{connection_id}' has no complete catalog"
            )));
        }
        transaction.execute(
            "INSERT OR IGNORE INTO profile_mcp_connections(profile_id, connection_id)
             VALUES (?1, ?2)",
            params![profile_id, connection_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn profile_connection_ids(
        &self,
        profile_id: &str,
    ) -> Result<Vec<String>, McpHostError> {
        validate_identity("profile", profile_id)?;
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT connection_id FROM profile_mcp_connections
             WHERE profile_id = ?1 ORDER BY connection_id",
        )?;
        let identifiers = statement
            .query_map([profile_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(identifiers)
    }

    pub(crate) fn profile_connection_statuses(
        &self,
        profile_id: &str,
    ) -> Result<Vec<McpConnectionStatus>, McpHostError> {
        validate_identity("profile", profile_id)?;
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT configured.connection_id, configured.integration_id,
                    configured.auth_kind, catalog.connection_id IS NOT NULL,
                    (SELECT count(*) FROM mcp_tools AS tool
                     WHERE tool.connection_id = configured.connection_id),
                    (SELECT count(*) FROM mcp_rejected_tools AS rejected
                     WHERE rejected.connection_id = configured.connection_id),
                    EXISTS(
                        SELECT 1 FROM profile_mcp_connections AS binding
                        WHERE binding.profile_id = ?1
                          AND binding.connection_id = configured.connection_id
                    )
             FROM mcp_connections AS configured
             LEFT JOIN mcp_catalogs AS catalog
               ON catalog.connection_id = configured.connection_id
             ORDER BY configured.connection_id",
        )?;
        let stored = statement
            .query_map([profile_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, bool>(6)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        stored
            .into_iter()
            .map(
                |(
                    connection,
                    integration,
                    credential,
                    catalog_loaded,
                    tools,
                    rejected_tools,
                    enabled,
                )| {
                    let auth = McpConnectionAuthKind::from_stored(&credential)?;
                    Ok(McpConnectionStatus {
                        connection,
                        integration,
                        auth,
                        registered: true,
                        catalog_loaded,
                        enabled_for_profile: enabled,
                        tools: usize::try_from(tools).map_err(|error| {
                            McpHostError::Invalid(format!(
                                "stored MCP tool count is invalid: {error}"
                            ))
                        })?,
                        rejected_tools: usize::try_from(rejected_tools).map_err(|error| {
                            McpHostError::Invalid(format!(
                                "stored rejected MCP tool count is invalid: {error}"
                            ))
                        })?,
                    })
                },
            )
            .collect()
    }

    pub(crate) fn disable_profile_connection(
        &self,
        profile_id: &str,
        connection_id: &str,
    ) -> Result<bool, McpHostError> {
        validate_identity("profile", profile_id)?;
        validate_identity("connection", connection_id)?;
        let mut connection = self.connection()?;
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let catalog_retained = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM mcp_catalogs WHERE connection_id = ?1
                 )
                 FROM mcp_connections WHERE connection_id = ?1",
                [connection_id],
                |row| row.get::<_, bool>(0),
            )
            .optional()?
            .ok_or_else(|| {
                McpHostError::NotFound(format!("connection '{connection_id}' is not registered"))
            })?;
        transaction.execute(
            "DELETE FROM profile_mcp_connections
             WHERE profile_id = ?1 AND connection_id = ?2",
            params![profile_id, connection_id],
        )?;
        transaction.commit()?;
        Ok(catalog_retained)
    }

    pub(crate) fn profile_tool_summaries(
        &self,
        profile_id: &str,
    ) -> Result<Vec<McpToolSummary>, McpHostError> {
        validate_identity("profile", profile_id)?;
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT connection.integration_id, binding.connection_id,
                    catalog.catalog_digest, tool.name, tool.description
             FROM profile_mcp_connections AS binding
             JOIN mcp_connections AS connection
               ON connection.connection_id = binding.connection_id
             JOIN mcp_catalogs AS catalog
               ON catalog.connection_id = binding.connection_id
             JOIN mcp_tools AS tool
               ON tool.connection_id = binding.connection_id
             WHERE binding.profile_id = ?1
             ORDER BY binding.connection_id, tool.name",
        )?;
        let tools = statement
            .query_map([profile_id], |row| {
                Ok(McpToolSummary {
                    integration_id: row.get(0)?,
                    connection_id: row.get(1)?,
                    catalog_digest: row.get(2)?,
                    name: row.get(3)?,
                    description: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tools)
    }

    pub(crate) fn resolve_profile_tools(
        &self,
        profile_id: &str,
        references: &[McpToolReference],
    ) -> Result<Vec<ResolvedMcpTool>, McpHostError> {
        validate_identity("profile", profile_id)?;
        if references.is_empty() {
            return Err(McpHostError::Invalid(
                "at least one MCP tool reference is required".to_owned(),
            ));
        }
        let mut connection = self.connection()?;
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)?;
        let mut catalogs = HashMap::new();
        let mut tools = Vec::with_capacity(references.len());
        for reference in references {
            let enabled = enabled_connection(&transaction, profile_id, reference.connection_id())?;
            if !catalogs.contains_key(reference.connection_id()) {
                let catalog =
                    load_catalog(&transaction, reference.connection_id())?.ok_or_else(|| {
                        McpHostError::NotFound(format!(
                            "connection '{}' has no complete catalog",
                            reference.connection_id()
                        ))
                    })?;
                catalogs.insert(reference.connection_id().to_owned(), catalog);
            }
            let catalog = catalogs.get(reference.connection_id()).ok_or_else(|| {
                McpHostError::Invalid("MCP catalog disappeared during one read".to_owned())
            })?;
            if catalog.digest() != reference.catalog_digest() {
                return Err(McpHostError::Conflict(format!(
                    "MCP tool reference for '{}/{}' is stale; search for the tool again",
                    reference.connection_id(),
                    reference.tool_name()
                )));
            }
            let tool = catalog
                .tools()
                .iter()
                .find(|tool| tool.name() == reference.tool_name())
                .cloned()
                .ok_or_else(|| {
                    McpHostError::NotFound(format!(
                        "tool '{}/{}' is absent from the referenced catalog",
                        reference.connection_id(),
                        reference.tool_name()
                    ))
                })?;
            let auth = McpConnectionAuth::from_stored(
                &enabled.auth_kind,
                enabled.auth_hostname,
                enabled.auth_account,
                enabled.auth_credential_id,
                enabled.oauth_registration,
                enabled.auth_header,
                enabled.auth_prefix,
            )?;
            auth.validate_oauth_binding(reference.connection_id(), catalog.endpoint())?;
            tools.push(ResolvedMcpTool {
                integration_id: enabled.integration_id,
                connection_id: reference.connection_id().to_owned(),
                endpoint: catalog.endpoint().to_owned(),
                request_headers: catalog.request_headers.clone(),
                protocol_version: catalog.protocol_version().to_owned(),
                adapter_revision: catalog.adapter_revision().to_owned(),
                auth,
                tool,
            });
        }
        transaction.commit()?;
        Ok(tools)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum McpConnectionAuthKind {
    None,
    GhCli,
    SecretServiceBearer,
    SecretServiceHeader,
    #[serde(rename = "oauth")]
    OAuth,
}

impl McpConnectionAuthKind {
    fn from_stored(value: &str) -> Result<Self, McpHostError> {
        match value {
            "none" => Ok(Self::None),
            "gh_cli" => Ok(Self::GhCli),
            "secret_service_bearer" => Ok(Self::SecretServiceBearer),
            "secret_service_header" => Ok(Self::SecretServiceHeader),
            "oauth" => Ok(Self::OAuth),
            _ => Err(McpHostError::Invalid(
                "stored MCP credential kind is malformed".to_owned(),
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct McpConnectionStatus {
    connection: String,
    integration: String,
    auth: McpConnectionAuthKind,
    registered: bool,
    catalog_loaded: bool,
    enabled_for_profile: bool,
    tools: usize,
    rejected_tools: usize,
}

struct EnabledConnection {
    integration_id: String,
    auth_kind: String,
    auth_hostname: Option<String>,
    auth_account: Option<String>,
    auth_credential_id: Option<String>,
    oauth_registration: Option<String>,
    auth_header: Option<String>,
    auth_prefix: Option<String>,
}

fn enabled_connection(
    transaction: &Transaction<'_>,
    profile_id: &str,
    connection_id: &str,
) -> Result<EnabledConnection, McpHostError> {
    transaction
        .query_row(
            "SELECT connection.integration_id, connection.auth_kind,
                    connection.auth_hostname, connection.auth_account,
                    connection.auth_credential_id, connection.oauth_registration_json,
                    connection.auth_header_name, connection.auth_header_prefix
             FROM profile_mcp_connections AS binding
             JOIN mcp_connections AS connection
               ON connection.connection_id = binding.connection_id
             WHERE binding.profile_id = ?1 AND binding.connection_id = ?2",
            params![profile_id, connection_id],
            |row| {
                Ok(EnabledConnection {
                    integration_id: row.get(0)?,
                    auth_kind: row.get(1)?,
                    auth_hostname: row.get(2)?,
                    auth_account: row.get(3)?,
                    auth_credential_id: row.get(4)?,
                    oauth_registration: row.get(5)?,
                    auth_header: row.get(6)?,
                    auth_prefix: row.get(7)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| {
            McpHostError::NotFound(format!(
                "connection '{connection_id}' is not enabled for profile '{profile_id}'"
            ))
        })
}

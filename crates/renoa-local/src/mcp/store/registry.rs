use std::collections::HashMap;

use rusqlite::{OptionalExtension as _, Transaction, params};

use super::{McpCatalogStore, load_catalog};
use crate::mcp::{
    AlphaMcpTool, McpConnectionAuth, McpHostError,
    registry::{McpToolReference, McpToolSummary},
    validate_identity,
};

impl McpCatalogStore {
    pub(crate) fn enable_alpha_connection(
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

    pub(crate) fn alpha_connection_ids(
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

    pub(crate) fn alpha_tool_summaries(
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

    pub(crate) fn resolve_alpha_tools(
        &self,
        profile_id: &str,
        references: &[McpToolReference],
    ) -> Result<Vec<AlphaMcpTool>, McpHostError> {
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
            )?;
            auth.validate_oauth_binding(reference.connection_id(), catalog.endpoint())?;
            tools.push(AlphaMcpTool {
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

struct EnabledConnection {
    integration_id: String,
    auth_kind: String,
    auth_hostname: Option<String>,
    auth_account: Option<String>,
    auth_credential_id: Option<String>,
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
                    connection.auth_credential_id
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

mod load;
mod registry;
use std::path::PathBuf;

use super::{
    McpCatalogSnapshot, McpCatalogTool, McpConnectionAuth, McpHostError, validate_endpoint,
    validate_identity,
};
use load::load_catalog;
use rusqlite::{Connection, OptionalExtension as _, Transaction, TransactionBehavior, params};

#[derive(Clone)]
pub(crate) struct McpCatalogStore {
    path: PathBuf,
}

pub(crate) struct McpConnectionConfig {
    pub(crate) endpoint: String,
    pub(crate) auth: McpConnectionAuth,
}

impl McpCatalogStore {
    #[cfg(test)]
    pub(crate) fn initialize(path: PathBuf) -> Result<Self, McpHostError> {
        crate::host::catalog::initialize(&path)?;
        Ok(Self { path })
    }

    pub(crate) fn open(path: PathBuf) -> Result<Self, McpHostError> {
        crate::host::catalog::open_verified(&path)?;
        Ok(Self { path })
    }

    pub(crate) fn register_direct_connection(
        &self,
        integration_id: &str,
        connection_id: &str,
        endpoint: &str,
    ) -> Result<(), McpHostError> {
        validate_identity("integration", integration_id)?;
        validate_identity("connection", connection_id)?;
        validate_endpoint(endpoint)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_integration(&transaction, integration_id, endpoint)?;
        ensure_connection(
            &transaction,
            connection_id,
            integration_id,
            &McpConnectionAuth::None,
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn register_gh_cli_connection(
        &self,
        integration_id: &str,
        connection_id: &str,
        endpoint: &str,
        hostname: &str,
        account: &str,
    ) -> Result<(), McpHostError> {
        validate_identity("integration", integration_id)?;
        validate_identity("connection", connection_id)?;
        validate_endpoint(endpoint)?;
        let auth = McpConnectionAuth::gh_cli(hostname, account)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_integration(&transaction, integration_id, endpoint)?;
        ensure_connection(&transaction, connection_id, integration_id, &auth)?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn connection_config(
        &self,
        connection_id: &str,
    ) -> Result<McpConnectionConfig, McpHostError> {
        validate_identity("connection", connection_id)?;
        let stored = self
            .connection()?
            .query_row(
                "SELECT integration.endpoint, connection.auth_kind,
                        connection.auth_hostname, connection.auth_account
                 FROM mcp_connections AS connection
                 JOIN mcp_integrations AS integration
                   ON integration.integration_id = connection.integration_id
                 WHERE connection.connection_id = ?1",
                [connection_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                McpHostError::NotFound(format!("connection '{connection_id}' is not registered"))
            })?;
        Ok(McpConnectionConfig {
            endpoint: stored.0,
            auth: McpConnectionAuth::from_stored(&stored.1, stored.2, stored.3)?,
        })
    }

    #[cfg(test)]
    pub(crate) fn connection_endpoint(&self, connection_id: &str) -> Result<String, McpHostError> {
        self.connection_config(connection_id)
            .map(|config| config.endpoint)
    }

    pub(crate) fn publish_catalog(
        &self,
        snapshot: &McpCatalogSnapshot,
    ) -> Result<(), McpHostError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let configured_endpoint =
            require_connection_endpoint(&transaction, snapshot.connection_id())?;
        if configured_endpoint != snapshot.endpoint() {
            return Err(McpHostError::Invalid(format!(
                "catalog endpoint for connection '{}' differs from its registered endpoint",
                snapshot.connection_id()
            )));
        }
        transaction.execute(
            "INSERT INTO mcp_catalogs(
                connection_id, endpoint, protocol_version, adapter_revision, catalog_digest
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(connection_id) DO UPDATE SET
                endpoint = excluded.endpoint,
                protocol_version = excluded.protocol_version,
                adapter_revision = excluded.adapter_revision,
                catalog_digest = excluded.catalog_digest",
            params![
                snapshot.connection_id.as_str(),
                snapshot.endpoint.as_str(),
                snapshot.protocol_version.as_str(),
                snapshot.adapter_revision.as_str(),
                snapshot.digest.as_str(),
            ],
        )?;
        transaction.execute(
            "DELETE FROM mcp_tools WHERE connection_id = ?1",
            [snapshot.connection_id()],
        )?;
        transaction.execute(
            "DELETE FROM mcp_rejected_tools WHERE connection_id = ?1",
            [snapshot.connection_id()],
        )?;
        for tool in &snapshot.tools {
            insert_tool(&transaction, snapshot.connection_id(), tool)?;
        }
        for rejected in &snapshot.rejected_tools {
            transaction.execute(
                "INSERT INTO mcp_rejected_tools(
                    connection_id, source_index, name, reason
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    snapshot.connection_id.as_str(),
                    i64::try_from(rejected.index).map_err(|error| {
                        McpHostError::Invalid(format!(
                            "rejected tool index cannot be stored: {error}"
                        ))
                    })?,
                    rejected.name.as_deref(),
                    rejected.reason.as_str(),
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn load_catalog(
        &self,
        connection_id: &str,
    ) -> Result<McpCatalogSnapshot, McpHostError> {
        validate_identity("connection", connection_id)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let catalog = load_catalog(&transaction, connection_id)?.ok_or_else(|| {
            McpHostError::NotFound(format!(
                "connection '{connection_id}' has no complete catalog"
            ))
        })?;
        transaction.commit()?;
        Ok(catalog)
    }

    pub(super) fn connection(&self) -> Result<Connection, McpHostError> {
        Ok(crate::host::catalog::open_verified(&self.path)?)
    }

    #[cfg(test)]
    pub(super) fn path(&self) -> &std::path::Path {
        &self.path
    }
}

fn ensure_integration(
    transaction: &Transaction<'_>,
    integration_id: &str,
    endpoint: &str,
) -> Result<(), McpHostError> {
    let existing = transaction
        .query_row(
            "SELECT kind, endpoint FROM mcp_integrations WHERE integration_id = ?1",
            [integration_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    match existing {
        None => {
            transaction.execute(
                "INSERT INTO mcp_integrations(integration_id, kind, endpoint)
                 VALUES (?1, 'direct_streamable_http', ?2)",
                params![integration_id, endpoint],
            )?;
            Ok(())
        }
        Some((kind, stored_endpoint))
            if kind == "direct_streamable_http" && stored_endpoint == endpoint =>
        {
            Ok(())
        }
        Some(_) => Err(McpHostError::Conflict(format!(
            "integration '{integration_id}' already has different configuration"
        ))),
    }
}

fn ensure_connection(
    transaction: &Transaction<'_>,
    connection_id: &str,
    integration_id: &str,
    auth: &McpConnectionAuth,
) -> Result<(), McpHostError> {
    let existing = transaction
        .query_row(
            "SELECT integration_id, auth_kind, auth_hostname, auth_account
             FROM mcp_connections WHERE connection_id = ?1",
            [connection_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?;
    match existing {
        None => {
            transaction.execute(
                "INSERT INTO mcp_connections(
                    connection_id, integration_id, auth_kind, auth_hostname, auth_account
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    connection_id,
                    integration_id,
                    auth.stored_kind(),
                    auth.stored_hostname(),
                    auth.stored_account(),
                ],
            )?;
            Ok(())
        }
        Some((stored_integration, auth_kind, hostname, account)) => {
            let stored_auth = McpConnectionAuth::from_stored(&auth_kind, hostname, account)?;
            if stored_integration == integration_id && stored_auth == *auth {
                Ok(())
            } else {
                Err(McpHostError::Conflict(format!(
                    "connection '{connection_id}' already has different configuration"
                )))
            }
        }
    }
}

fn require_connection_endpoint(
    transaction: &Transaction<'_>,
    connection_id: &str,
) -> Result<String, McpHostError> {
    transaction
        .query_row(
            "SELECT integration.endpoint
             FROM mcp_connections AS connection
             JOIN mcp_integrations AS integration
               ON integration.integration_id = connection.integration_id
             WHERE connection.connection_id = ?1",
            [connection_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| {
            McpHostError::NotFound(format!("connection '{connection_id}' is not registered"))
        })
}

fn insert_tool(
    transaction: &Transaction<'_>,
    connection_id: &str,
    tool: &McpCatalogTool,
) -> Result<(), McpHostError> {
    transaction.execute(
        "INSERT INTO mcp_tools(
            connection_id, name, description, input_schema_json,
            model_input_schema_json, output_schema_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            connection_id,
            tool.name.as_str(),
            tool.description.as_str(),
            serde_json::to_string(&tool.input_schema)?,
            serde_json::to_string(&tool.model_input_schema)?,
            tool.output_schema
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        ],
    )?;
    Ok(())
}

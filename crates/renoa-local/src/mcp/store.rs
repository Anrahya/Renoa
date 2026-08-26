mod load;
mod schema;

use std::path::PathBuf;

use super::{
    AlphaMcpTool, McpCatalogSnapshot, McpCatalogTool, McpHostError, validate_endpoint,
    validate_identity,
};
use load::load_catalog;
use rusqlite::{Connection, OptionalExtension as _, Transaction, TransactionBehavior, params};

pub(crate) use schema::HOST_DATABASE;

#[derive(Clone)]
pub(crate) struct McpCatalogStore {
    path: PathBuf,
}

impl McpCatalogStore {
    pub(crate) fn initialize(path: PathBuf) -> Result<Self, McpHostError> {
        let mut connection = schema::open(&path)?;
        restrict_database_permissions(&path)?;
        schema::initialize(&mut connection)?;
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
        ensure_connection(&transaction, connection_id, integration_id)?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn connection_endpoint(&self, connection_id: &str) -> Result<String, McpHostError> {
        validate_identity("connection", connection_id)?;
        self.connection()?
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

    pub(crate) fn select_alpha_tool(
        &self,
        profile_id: &str,
        connection_id: &str,
        tool_name: &str,
    ) -> Result<(), McpHostError> {
        validate_identity("profile", profile_id)?;
        validate_identity("connection", connection_id)?;
        validate_identity("tool", tool_name)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists = transaction
            .query_row(
                "SELECT 1 FROM mcp_tools WHERE connection_id = ?1 AND name = ?2",
                params![connection_id, tool_name],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Err(McpHostError::NotFound(format!(
                "tool '{tool_name}' is absent from connection '{connection_id}'"
            )));
        }
        transaction.execute(
            "INSERT OR IGNORE INTO profile_mcp_tools(profile_id, connection_id, tool_name)
             VALUES (?1, ?2, ?3)",
            params![profile_id, connection_id, tool_name],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn alpha_tools(&self, profile_id: &str) -> Result<Vec<AlphaMcpTool>, McpHostError> {
        validate_identity("profile", profile_id)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let mut statement = transaction.prepare(
            "SELECT binding.connection_id, binding.tool_name, connection.integration_id
             FROM profile_mcp_tools AS binding
             JOIN mcp_connections AS connection
               ON connection.connection_id = binding.connection_id
             WHERE binding.profile_id = ?1
             ORDER BY binding.connection_id, binding.tool_name",
        )?;
        let bindings = statement
            .query_map([profile_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let mut loaded_catalog: Option<(String, McpCatalogSnapshot)> = None;
        let mut tools = Vec::with_capacity(bindings.len());
        for (connection_id, tool_name, integration_id) in bindings {
            if loaded_catalog
                .as_ref()
                .is_none_or(|(loaded_connection, _)| loaded_connection != &connection_id)
            {
                let catalog = load_catalog(&transaction, &connection_id)?.ok_or_else(|| {
                    McpHostError::NotFound(format!(
                        "selected connection '{connection_id}' has no complete catalog"
                    ))
                })?;
                loaded_catalog = Some((connection_id.clone(), catalog));
            }
            let Some((_, catalog)) = loaded_catalog.as_ref() else {
                return Err(McpHostError::Invalid(
                    "selected MCP catalog was not loaded".to_owned(),
                ));
            };
            let tool = catalog
                .tools
                .iter()
                .find(|tool| tool.name() == tool_name)
                .cloned()
                .ok_or_else(|| {
                    McpHostError::NotFound(format!(
                        "selected tool '{connection_id}/{tool_name}' is absent from the latest complete catalog"
                    ))
                })?;
            tools.push(AlphaMcpTool {
                integration_id,
                connection_id,
                endpoint: catalog.endpoint.clone(),
                protocol_version: catalog.protocol_version.clone(),
                adapter_revision: catalog.adapter_revision.clone(),
                tool,
            });
        }
        transaction.commit()?;
        Ok(tools)
    }

    fn connection(&self) -> Result<Connection, McpHostError> {
        let connection = schema::open(&self.path)?;
        schema::verify(&connection)?;
        Ok(connection)
    }

    #[cfg(test)]
    pub(super) fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[cfg(unix)]
fn restrict_database_permissions(path: &std::path::Path) -> Result<(), McpHostError> {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_database_permissions(_path: &std::path::Path) -> Result<(), McpHostError> {
    Ok(())
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
) -> Result<(), McpHostError> {
    let existing = transaction
        .query_row(
            "SELECT integration_id, auth_kind FROM mcp_connections WHERE connection_id = ?1",
            [connection_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    match existing {
        None => {
            transaction.execute(
                "INSERT INTO mcp_connections(connection_id, integration_id, auth_kind)
                 VALUES (?1, ?2, 'none')",
                params![connection_id, integration_id],
            )?;
            Ok(())
        }
        Some((stored_integration, auth_kind))
            if stored_integration == integration_id && auth_kind == "none" =>
        {
            Ok(())
        }
        Some(_) => Err(McpHostError::Conflict(format!(
            "connection '{connection_id}' already has different configuration"
        ))),
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

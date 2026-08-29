mod load;
mod registry;
use std::path::PathBuf;

pub(crate) use registry::McpConnectionStatus;

use super::{
    McpCatalogSnapshot, McpCatalogTool, McpConnectionAuth, McpHostError, McpRequestHeaders,
    validate_endpoint, validate_identity,
};
use load::load_catalog;
use rusqlite::{Connection, OptionalExtension as _, Transaction, TransactionBehavior, params};

#[derive(Clone)]
pub(crate) struct McpCatalogStore {
    path: PathBuf,
}

pub(crate) struct McpConnectionConfig {
    pub(crate) endpoint: String,
    pub(crate) request_headers: McpRequestHeaders,
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
        self.register_connection(
            integration_id,
            connection_id,
            endpoint,
            &McpRequestHeaders::default(),
            &McpConnectionAuth::None,
        )
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
        self.register_connection(
            integration_id,
            connection_id,
            endpoint,
            &McpRequestHeaders::default(),
            &auth,
        )
    }

    pub(crate) fn register_connection(
        &self,
        integration_id: &str,
        connection_id: &str,
        endpoint: &str,
        request_headers: &McpRequestHeaders,
        auth: &McpConnectionAuth,
    ) -> Result<(), McpHostError> {
        validate_identity("integration", integration_id)?;
        validate_identity("connection", connection_id)?;
        validate_endpoint(endpoint)?;
        auth.validate_oauth_binding(connection_id, endpoint)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_integration(&transaction, integration_id, endpoint, request_headers)?;
        ensure_connection(&transaction, connection_id, integration_id, auth)?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn replace_connection(
        &self,
        integration_id: &str,
        connection_id: &str,
        endpoint: &str,
        request_headers: &McpRequestHeaders,
        auth: &McpConnectionAuth,
    ) -> Result<(), McpHostError> {
        validate_identity("integration", integration_id)?;
        validate_identity("connection", connection_id)?;
        validate_endpoint(endpoint)?;
        auth.validate_oauth_binding(connection_id, endpoint)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_integration(&transaction, integration_id, endpoint, request_headers)?;
        match ensure_connection(&transaction, connection_id, integration_id, auth) {
            Ok(()) => {}
            Err(McpHostError::Conflict(_)) => {
                transaction.execute(
                    "DELETE FROM profile_mcp_connections WHERE connection_id = ?1",
                    [connection_id],
                )?;
                transaction.execute(
                    "DELETE FROM mcp_connections WHERE connection_id = ?1",
                    [connection_id],
                )?;
                ensure_connection(&transaction, connection_id, integration_id, auth)?;
            }
            Err(error) => return Err(error),
        }
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
                "SELECT integration.endpoint, integration.request_headers_json,
                        connection.auth_kind, connection.auth_hostname,
                        connection.auth_account, connection.auth_credential_id,
                        connection.oauth_registration_json
                 FROM mcp_connections AS connection
                 JOIN mcp_integrations AS integration
                   ON integration.integration_id = connection.integration_id
                 WHERE connection.connection_id = ?1",
                [connection_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                McpHostError::NotFound(format!("connection '{connection_id}' is not registered"))
            })?;
        let endpoint = stored.0;
        let auth =
            McpConnectionAuth::from_stored(&stored.2, stored.3, stored.4, stored.5, stored.6)?;
        auth.validate_oauth_binding(connection_id, &endpoint)?;
        Ok(McpConnectionConfig {
            endpoint,
            request_headers: McpRequestHeaders::from_stored(&stored.1)?,
            auth,
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
        let (configured_endpoint, configured_headers) =
            require_connection_configuration(&transaction, snapshot.connection_id())?;
        if configured_endpoint != snapshot.endpoint()
            || configured_headers.values() != snapshot.request_headers()
        {
            return Err(McpHostError::Invalid(format!(
                "catalog transport configuration for connection '{}' differs from its registration",
                snapshot.connection_id()
            )));
        }
        store_catalog(&transaction, snapshot)?;
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn publish_and_enable_connection(
        &self,
        profile_id: &str,
        snapshot: &McpCatalogSnapshot,
    ) -> Result<(), McpHostError> {
        validate_identity("profile", profile_id)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (configured_endpoint, configured_headers) =
            require_connection_configuration(&transaction, snapshot.connection_id())?;
        if configured_endpoint != snapshot.endpoint()
            || configured_headers.values() != snapshot.request_headers()
        {
            return Err(McpHostError::Invalid(format!(
                "catalog transport configuration for connection '{}' differs from its registration",
                snapshot.connection_id()
            )));
        }
        store_catalog(&transaction, snapshot)?;
        transaction.execute(
            "INSERT OR IGNORE INTO profile_mcp_connections(profile_id, connection_id)
             VALUES (?1, ?2)",
            params![profile_id, snapshot.connection_id()],
        )?;
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

    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }
}

fn store_catalog(
    transaction: &Transaction<'_>,
    snapshot: &McpCatalogSnapshot,
) -> Result<(), McpHostError> {
    transaction.execute(
        "INSERT INTO mcp_catalogs(
                connection_id, endpoint, request_headers_json, protocol_version,
                adapter_revision, catalog_digest
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(connection_id) DO UPDATE SET
                endpoint = excluded.endpoint,
                request_headers_json = excluded.request_headers_json,
                protocol_version = excluded.protocol_version,
                adapter_revision = excluded.adapter_revision,
                catalog_digest = excluded.catalog_digest",
        params![
            snapshot.connection_id.as_str(),
            snapshot.endpoint.as_str(),
            snapshot.request_headers.encoded()?,
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
        insert_tool(transaction, snapshot.connection_id(), tool)?;
    }
    for rejected in &snapshot.rejected_tools {
        transaction.execute(
            "INSERT INTO mcp_rejected_tools(
                    connection_id, source_index, name, reason
                 ) VALUES (?1, ?2, ?3, ?4)",
            params![
                snapshot.connection_id.as_str(),
                i64::try_from(rejected.index).map_err(|error| {
                    McpHostError::Invalid(format!("rejected tool index cannot be stored: {error}"))
                })?,
                rejected.name.as_deref(),
                rejected.reason.as_str(),
            ],
        )?;
    }
    Ok(())
}

fn ensure_integration(
    transaction: &Transaction<'_>,
    integration_id: &str,
    endpoint: &str,
    request_headers: &McpRequestHeaders,
) -> Result<(), McpHostError> {
    let existing = transaction
        .query_row(
            "SELECT kind, endpoint, request_headers_json
             FROM mcp_integrations WHERE integration_id = ?1",
            [integration_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    match existing {
        None => {
            transaction.execute(
                "INSERT INTO mcp_integrations(
                    integration_id, kind, endpoint, request_headers_json
                 ) VALUES (?1, 'direct_streamable_http', ?2, ?3)",
                params![integration_id, endpoint, request_headers.encoded()?],
            )?;
            Ok(())
        }
        Some((kind, stored_endpoint, stored_headers))
            if kind == "direct_streamable_http"
                && stored_endpoint == endpoint
                && McpRequestHeaders::from_stored(&stored_headers)? == *request_headers =>
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
            "SELECT integration_id, auth_kind, auth_hostname, auth_account,
                    auth_credential_id, oauth_registration_json
             FROM mcp_connections WHERE connection_id = ?1",
            [connection_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()?;
    match existing {
        None => {
            transaction.execute(
                "INSERT INTO mcp_connections(
                    connection_id, integration_id, auth_kind, auth_hostname, auth_account,
                    auth_credential_id, oauth_registration_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    connection_id,
                    integration_id,
                    auth.stored_kind(),
                    auth.stored_hostname(),
                    auth.stored_account(),
                    auth.stored_credential_id(),
                    auth.stored_oauth_registration()?,
                ],
            )?;
            Ok(())
        }
        Some((
            stored_integration,
            auth_kind,
            hostname,
            account,
            credential_id,
            oauth_registration,
        )) => {
            let stored_auth = McpConnectionAuth::from_stored(
                &auth_kind,
                hostname,
                account,
                credential_id,
                oauth_registration,
            )?;
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

fn require_connection_configuration(
    transaction: &Transaction<'_>,
    connection_id: &str,
) -> Result<(String, McpRequestHeaders), McpHostError> {
    transaction
        .query_row(
            "SELECT integration.endpoint, integration.request_headers_json
             FROM mcp_connections AS connection
             JOIN mcp_integrations AS integration
               ON integration.integration_id = connection.integration_id
             WHERE connection.connection_id = ?1",
            [connection_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
        .ok_or_else(|| {
            McpHostError::NotFound(format!("connection '{connection_id}' is not registered"))
        })
        .and_then(|(endpoint, headers)| Ok((endpoint, McpRequestHeaders::from_stored(&headers)?)))
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

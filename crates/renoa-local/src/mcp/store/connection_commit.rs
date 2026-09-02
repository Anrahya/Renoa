use rusqlite::{TransactionBehavior, params};

use super::{McpCatalogStore, ensure_connection, ensure_integration, store_catalog};
use crate::mcp::{
    McpCatalogSnapshot, McpConnectionAuth, McpHostError, McpRequestHeaders, validate_endpoint,
    validate_identity,
};

#[derive(Clone)]
pub(crate) struct McpConnectionCandidate {
    integration_id: String,
    connection_id: String,
    endpoint: String,
    request_headers: McpRequestHeaders,
    auth: McpConnectionAuth,
}

impl McpConnectionCandidate {
    pub(crate) fn new(
        integration_id: String,
        connection_id: String,
        endpoint: String,
        request_headers: McpRequestHeaders,
        auth: McpConnectionAuth,
    ) -> Result<Self, McpHostError> {
        validate_identity("integration", &integration_id)?;
        validate_identity("connection", &connection_id)?;
        validate_endpoint(&endpoint)?;
        auth.validate_oauth_binding(&connection_id, &endpoint)?;
        Ok(Self {
            integration_id,
            connection_id,
            endpoint,
            request_headers,
            auth,
        })
    }

    pub(crate) fn connection_id(&self) -> &str {
        &self.connection_id
    }

    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub(crate) const fn request_headers(&self) -> &McpRequestHeaders {
        &self.request_headers
    }

    pub(crate) const fn auth(&self) -> &McpConnectionAuth {
        &self.auth
    }
}

impl McpCatalogStore {
    pub(crate) fn preflight_connection(
        &self,
        candidate: &McpConnectionCandidate,
        replace: bool,
    ) -> Result<(), McpHostError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_integration(
            &transaction,
            &candidate.integration_id,
            &candidate.endpoint,
            &candidate.request_headers,
        )?;
        match ensure_connection(
            &transaction,
            &candidate.connection_id,
            &candidate.integration_id,
            &candidate.auth,
        ) {
            Ok(()) => {}
            Err(McpHostError::Conflict(_)) if replace => {}
            Err(error) => return Err(error),
        }
        transaction.rollback()?;
        Ok(())
    }

    pub(crate) fn commit_connection(
        &self,
        profile_id: &str,
        candidate: &McpConnectionCandidate,
        snapshot: &McpCatalogSnapshot,
        replace: bool,
    ) -> Result<(), McpHostError> {
        validate_identity("profile", profile_id)?;
        if snapshot.connection_id() != candidate.connection_id
            || snapshot.endpoint() != candidate.endpoint
            || snapshot.request_headers() != candidate.request_headers.values()
        {
            return Err(McpHostError::Invalid(
                "discovered MCP catalog differs from the proposed connection configuration"
                    .to_owned(),
            ));
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_integration(
            &transaction,
            &candidate.integration_id,
            &candidate.endpoint,
            &candidate.request_headers,
        )?;
        match ensure_connection(
            &transaction,
            &candidate.connection_id,
            &candidate.integration_id,
            &candidate.auth,
        ) {
            Ok(()) => {}
            Err(McpHostError::Conflict(_)) if replace => {
                replace_connection(&transaction, candidate)?;
            }
            Err(error) => return Err(error),
        }
        store_catalog(&transaction, snapshot)?;
        transaction.execute(
            "INSERT OR IGNORE INTO profile_mcp_connections(profile_id, connection_id)
             VALUES (?1, ?2)",
            params![profile_id, candidate.connection_id],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

fn replace_connection(
    transaction: &rusqlite::Transaction<'_>,
    candidate: &McpConnectionCandidate,
) -> Result<(), McpHostError> {
    transaction.execute(
        "DELETE FROM mcp_oauth_flows WHERE connection_id = ?1",
        [&candidate.connection_id],
    )?;
    transaction.execute(
        "DELETE FROM mcp_oauth_receipts WHERE connection_id = ?1",
        [&candidate.connection_id],
    )?;
    let changed = transaction.execute(
        "UPDATE mcp_connections
         SET integration_id = ?2,
             auth_kind = ?3,
             auth_hostname = ?4,
             auth_account = ?5,
             auth_credential_id = ?6,
             oauth_registration_json = ?7,
             auth_header_name = ?8,
             auth_header_prefix = ?9
         WHERE connection_id = ?1",
        params![
            candidate.connection_id,
            candidate.integration_id,
            candidate.auth.stored_kind(),
            candidate.auth.stored_hostname(),
            candidate.auth.stored_account(),
            candidate.auth.stored_credential_id(),
            candidate.auth.stored_oauth_registration()?,
            candidate.auth.stored_header(),
            candidate.auth.stored_prefix(),
        ],
    )?;
    if changed != 1 {
        return Err(McpHostError::Invalid(
            "MCP connection disappeared during replacement".to_owned(),
        ));
    }
    Ok(())
}

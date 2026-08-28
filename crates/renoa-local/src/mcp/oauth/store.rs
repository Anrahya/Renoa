use std::path::PathBuf;

use rusqlite::{OptionalExtension as _, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::mcp::{McpHostError, validate_identity};

const MAX_RECEIPT_BYTES: usize = 16 * 1_024;

#[derive(Clone)]
pub(super) struct OAuthFlowStore {
    database: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OAuthPhase {
    BeginInFlight,
    AwaitingCallback,
    CallbackReady,
    ExchangeInFlight,
    RefreshInFlight,
    Unknown,
}

impl OAuthPhase {
    const fn stored(self) -> &'static str {
        match self {
            Self::BeginInFlight => "begin_in_flight",
            Self::AwaitingCallback => "awaiting_callback",
            Self::CallbackReady => "callback_ready",
            Self::ExchangeInFlight => "exchange_in_flight",
            Self::RefreshInFlight => "refresh_in_flight",
            Self::Unknown => "unknown",
        }
    }

    fn from_stored(value: &str) -> Result<Self, McpHostError> {
        match value {
            "begin_in_flight" => Ok(Self::BeginInFlight),
            "awaiting_callback" => Ok(Self::AwaitingCallback),
            "callback_ready" => Ok(Self::CallbackReady),
            "exchange_in_flight" => Ok(Self::ExchangeInFlight),
            "refresh_in_flight" => Ok(Self::RefreshInFlight),
            "unknown" => Ok(Self::Unknown),
            _ => Err(McpHostError::Invalid(
                "stored MCP OAuth phase is malformed".to_owned(),
            )),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct OAuthFlow {
    pub(super) connection_id: String,
    pub(super) operation_id: String,
    pub(super) phase: OAuthPhase,
    pub(super) callback_port: Option<u16>,
    pub(super) expires_at_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OAuthReceipt {
    Authorized,
    RemoteFailure,
    CallbackRejected { error: String },
    AuthorizationRequired,
}

impl OAuthReceipt {
    fn validate(&self) -> Result<(), McpHostError> {
        match self {
            Self::CallbackRejected { error }
                if error.is_empty()
                    || error.len() > 128
                    || !error.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                    }) =>
            {
                Err(McpHostError::Invalid(
                    "stored OAuth callback rejection is malformed".to_owned(),
                ))
            }
            Self::Authorized
            | Self::RemoteFailure
            | Self::CallbackRejected { .. }
            | Self::AuthorizationRequired => Ok(()),
        }
    }
}

impl OAuthFlow {
    pub(super) fn interactive(
        connection_id: &str,
        operation_id: &str,
        phase: OAuthPhase,
        callback_port: u16,
        expires_at_ms: i64,
    ) -> Result<Self, McpHostError> {
        validate_flow_identity(connection_id, operation_id)?;
        if !matches!(
            phase,
            OAuthPhase::BeginInFlight
                | OAuthPhase::AwaitingCallback
                | OAuthPhase::CallbackReady
                | OAuthPhase::ExchangeInFlight
        ) {
            return Err(McpHostError::Invalid(
                "interactive MCP OAuth flow has a non-interactive phase".to_owned(),
            ));
        }
        if expires_at_ms <= 0 {
            return Err(McpHostError::Invalid(
                "MCP OAuth callback expiry must be positive".to_owned(),
            ));
        }
        Ok(Self {
            connection_id: connection_id.to_owned(),
            operation_id: operation_id.to_owned(),
            phase,
            callback_port: Some(callback_port),
            expires_at_ms: Some(expires_at_ms),
        })
    }

    pub(super) fn non_interactive(
        connection_id: &str,
        operation_id: &str,
        phase: OAuthPhase,
    ) -> Result<Self, McpHostError> {
        validate_flow_identity(connection_id, operation_id)?;
        if !matches!(phase, OAuthPhase::RefreshInFlight | OAuthPhase::Unknown) {
            return Err(McpHostError::Invalid(
                "non-interactive MCP OAuth flow has an interactive phase".to_owned(),
            ));
        }
        Ok(Self {
            connection_id: connection_id.to_owned(),
            operation_id: operation_id.to_owned(),
            phase,
            callback_port: None,
            expires_at_ms: None,
        })
    }

    pub(super) fn with_phase(&self, phase: OAuthPhase) -> Result<Self, McpHostError> {
        match (self.callback_port, self.expires_at_ms) {
            (Some(port), Some(expiry)) => {
                Self::interactive(&self.connection_id, &self.operation_id, phase, port, expiry)
            }
            (None, None) => Self::non_interactive(&self.connection_id, &self.operation_id, phase),
            _ => Err(McpHostError::Invalid(
                "stored MCP OAuth flow has incomplete callback state".to_owned(),
            )),
        }
    }
}

impl OAuthFlowStore {
    pub(super) const fn new(database: PathBuf) -> Self {
        Self { database }
    }

    pub(super) async fn load(
        &self,
        connection_id: &str,
    ) -> Result<Option<OAuthFlow>, McpHostError> {
        validate_identity("connection", connection_id)?;
        let database = self.database.clone();
        let connection_id = connection_id.to_owned();
        tokio::task::spawn_blocking(move || load(&database, &connection_id)).await?
    }

    pub(super) async fn put(&self, flow: &OAuthFlow) -> Result<(), McpHostError> {
        let database = self.database.clone();
        let flow = flow.clone();
        tokio::task::spawn_blocking(move || put(&database, &flow)).await?
    }

    pub(super) async fn delete(&self, connection_id: &str) -> Result<(), McpHostError> {
        validate_identity("connection", connection_id)?;
        let database = self.database.clone();
        let connection_id = connection_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let connection = crate::host::catalog::open_verified(&database)?;
            connection.execute(
                "DELETE FROM mcp_oauth_flows WHERE connection_id = ?1",
                [&connection_id],
            )?;
            Ok(())
        })
        .await?
    }

    pub(super) async fn receipt(
        &self,
        connection_id: &str,
        operation_id: &str,
    ) -> Result<Option<OAuthReceipt>, McpHostError> {
        validate_flow_identity(connection_id, operation_id)?;
        let database = self.database.clone();
        let connection_id = connection_id.to_owned();
        let operation_id = operation_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let connection = crate::host::catalog::open_verified(&database)?;
            let encoded = connection
                .query_row(
                    "SELECT outcome_json FROM mcp_oauth_receipts
                     WHERE connection_id = ?1 AND operation_id = ?2",
                    params![connection_id, operation_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            encoded.as_deref().map(decode_receipt).transpose()
        })
        .await?
    }

    pub(super) async fn put_receipt(
        &self,
        connection_id: &str,
        operation_id: &str,
        receipt: &OAuthReceipt,
    ) -> Result<(), McpHostError> {
        validate_flow_identity(connection_id, operation_id)?;
        receipt.validate()?;
        let encoded = serde_json::to_string(receipt)?;
        if encoded.len() > MAX_RECEIPT_BYTES {
            return Err(McpHostError::Invalid(
                "MCP OAuth receipt exceeds its storage boundary".to_owned(),
            ));
        }
        let database = self.database.clone();
        let connection_id = connection_id.to_owned();
        let operation_id = operation_id.to_owned();
        tokio::task::spawn_blocking(move || {
            let mut connection = crate::host::catalog::open_verified(&database)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute(
                "INSERT OR IGNORE INTO mcp_oauth_receipts(
                     connection_id, operation_id, outcome_json
                 ) VALUES (?1, ?2, ?3)",
                params![connection_id, operation_id, encoded],
            )?;
            let stored = transaction.query_row(
                "SELECT outcome_json FROM mcp_oauth_receipts
                 WHERE connection_id = ?1 AND operation_id = ?2",
                params![connection_id, operation_id],
                |row| row.get::<_, String>(0),
            )?;
            if stored != encoded {
                return Err(McpHostError::Conflict(
                    "MCP OAuth operation identity already has a different terminal outcome"
                        .to_owned(),
                ));
            }
            transaction.commit()?;
            Ok(())
        })
        .await?
    }
}

fn decode_receipt(encoded: &str) -> Result<OAuthReceipt, McpHostError> {
    if encoded.len() > MAX_RECEIPT_BYTES {
        return Err(McpHostError::Invalid(
            "stored MCP OAuth receipt exceeds its boundary".to_owned(),
        ));
    }
    let receipt = serde_json::from_str::<OAuthReceipt>(encoded)?;
    receipt.validate()?;
    Ok(receipt)
}

fn load(
    database: &std::path::Path,
    connection_id: &str,
) -> Result<Option<OAuthFlow>, McpHostError> {
    let connection = crate::host::catalog::open_verified(database)?;
    let stored = connection
        .query_row(
            "SELECT operation_id, phase, callback_port, expires_at_ms
             FROM mcp_oauth_flows WHERE connection_id = ?1",
            [connection_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<u16>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .optional()?;
    stored
        .map(|(operation_id, phase, callback_port, expires_at_ms)| {
            let phase = OAuthPhase::from_stored(&phase)?;
            let flow = OAuthFlow {
                connection_id: connection_id.to_owned(),
                operation_id,
                phase,
                callback_port,
                expires_at_ms,
            };
            validate_loaded(&flow)?;
            Ok(flow)
        })
        .transpose()
}

fn put(database: &std::path::Path, flow: &OAuthFlow) -> Result<(), McpHostError> {
    validate_loaded(flow)?;
    let connection = crate::host::catalog::open_verified(database)?;
    connection.execute(
        "INSERT INTO mcp_oauth_flows(
             connection_id, operation_id, phase, callback_port, expires_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(connection_id) DO UPDATE SET
             operation_id = excluded.operation_id,
             phase = excluded.phase,
             callback_port = excluded.callback_port,
             expires_at_ms = excluded.expires_at_ms",
        params![
            flow.connection_id,
            flow.operation_id,
            flow.phase.stored(),
            flow.callback_port,
            flow.expires_at_ms,
        ],
    )?;
    Ok(())
}

fn validate_flow_identity(connection_id: &str, operation_id: &str) -> Result<(), McpHostError> {
    validate_identity("connection", connection_id)?;
    if operation_id.is_empty() || operation_id.len() > 512 {
        return Err(McpHostError::Invalid(
            "MCP OAuth operation id must be 1-512 bytes".to_owned(),
        ));
    }
    Ok(())
}

fn validate_loaded(flow: &OAuthFlow) -> Result<(), McpHostError> {
    validate_flow_identity(&flow.connection_id, &flow.operation_id)?;
    match (flow.phase, flow.callback_port, flow.expires_at_ms) {
        (
            OAuthPhase::BeginInFlight
            | OAuthPhase::AwaitingCallback
            | OAuthPhase::CallbackReady
            | OAuthPhase::ExchangeInFlight,
            Some(_),
            Some(expiry),
        ) if expiry > 0 => Ok(()),
        (OAuthPhase::RefreshInFlight | OAuthPhase::Unknown, None, None) => Ok(()),
        _ => Err(McpHostError::Invalid(
            "stored MCP OAuth flow is malformed".to_owned(),
        )),
    }
}

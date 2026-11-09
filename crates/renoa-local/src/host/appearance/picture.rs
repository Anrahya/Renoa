//! A picture the owner sets over an agent's look. The Host stores the bytes
//! whole in its catalog, so setting one and its receipt commit together and a
//! refusal leaves nothing behind; it checks the bytes' signature against the
//! declared type and decodes nothing.

use renoa_kernel::AgentId;
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{host::catalog::HostCatalogError, workspace::hex_sha256};

/// The largest picture: one that fits a Host call in base64, with room left,
/// for an app that shrinks the image before it sends it.
pub const MAX_PICTURE_BYTES: usize = 256 * 1024;

/// The picture types the Host keeps, each with the bytes its files begin with.
const TYPES: [(&str, &[u8]); 3] = [
    ("image/png", b"\x89PNG\r\n\x1a\n"),
    ("image/jpeg", b"\xff\xd8\xff"),
    ("image/webp", b"RIFF"),
];

/// A picture as the Host keeps it. `revision` is the SHA-256 of `bytes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPicture {
    pub revision: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// A picture to set, as the owner sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPicture {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// One change of an agent's picture, made once under `operation_id`. It
/// applies only while the agent's picture is still `expected_revision`;
/// `picture: None` takes the picture away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PictureChange {
    pub operation_id: Uuid,
    pub expected_revision: Option<String>,
    pub picture: Option<NewPicture>,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PictureError {
    /// The change is malformed; the text says what was sent and what fits.
    #[error("{0}")]
    Invalid(String),
    /// The agent's picture is no longer the one the change expected.
    #[error("the agent's picture changed: it is now {}", current.as_deref().unwrap_or("none"))]
    Stale { current: Option<String> },
    /// The operation id was already used for another change.
    #[error("operation {0} was already used for another picture change")]
    OperationReused(Uuid),
    #[error(transparent)]
    Catalog(#[from] HostCatalogError),
    #[error(transparent)]
    Host(#[from] crate::LocalHostError),
}

impl From<rusqlite::Error> for PictureError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Catalog(error.into())
    }
}

/// What a receipt records: the change, by the revision it sets, never the
/// bytes.
#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct Request {
    agent_id: String,
    expected_revision: Option<String>,
    revision: Option<String>,
    content_type: Option<String>,
}

/// Schema 54's picture tables: a row only while a picture is set, and one
/// receipt per change.
pub(super) fn initialize(transaction: &Transaction<'_>) -> Result<(), HostCatalogError> {
    let types: Vec<String> = TYPES.iter().map(|(name, _)| format!("'{name}'")).collect();
    transaction.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS host_agent_pictures (
            agent_id TEXT PRIMARY KEY REFERENCES host_agents(agent_id),
            content_type TEXT NOT NULL CHECK (content_type IN ({})),
            bytes BLOB NOT NULL CHECK (length(bytes) BETWEEN 1 AND {MAX_PICTURE_BYTES}),
            revision TEXT NOT NULL CHECK (length(revision) = 64)
        ) STRICT;
        CREATE TABLE IF NOT EXISTS host_agent_picture_operations (
            operation_id TEXT PRIMARY KEY CHECK (length(operation_id) = 36),
            agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
            request_json TEXT NOT NULL CHECK (json_valid(request_json)),
            result_json TEXT NOT NULL CHECK (json_valid(result_json))
        ) STRICT;",
        types.join(", ")
    ))?;
    Ok(())
}

/// Checks `picture` before anything is stored, and returns its revision.
pub(super) fn validate(picture: &NewPicture) -> Result<String, PictureError> {
    let Some((_, signature)) = TYPES.iter().find(|(name, _)| *name == picture.content_type) else {
        return Err(PictureError::Invalid(format!(
            "content_type `{}` is not a picture type the Host keeps; send image/png, image/jpeg or image/webp",
            picture.content_type
        )));
    };
    if picture.bytes.is_empty() || picture.bytes.len() > MAX_PICTURE_BYTES {
        return Err(PictureError::Invalid(format!(
            "the picture is {} bytes; send 1 to {MAX_PICTURE_BYTES} bytes, shrinking it first",
            picture.bytes.len()
        )));
    }
    let webp = picture.content_type == "image/webp";
    let signed = picture.bytes.starts_with(signature)
        && (!webp || picture.bytes.get(8..12) == Some(b"WEBP".as_slice()));
    if !signed {
        return Err(PictureError::Invalid(format!(
            "the bytes are not a {} file; send the file's own bytes under its own type",
            picture.content_type
        )));
    }
    Ok(hex_sha256(&picture.bytes))
}

/// `agent`'s picture, if one is set.
pub(super) fn read(
    connection: &Connection,
    agent: AgentId,
) -> Result<Option<AgentPicture>, HostCatalogError> {
    Ok(connection
        .query_row(
            "SELECT revision, content_type, bytes FROM host_agent_pictures WHERE agent_id = ?1",
            [agent.to_string()],
            |row| {
                Ok(AgentPicture {
                    revision: row.get(0)?,
                    content_type: row.get(1)?,
                    bytes: row.get(2)?,
                })
            },
        )
        .optional()?)
}

/// `agent`'s picture revision, if one is set.
pub(super) fn revision(
    connection: &Connection,
    agent: AgentId,
) -> Result<Option<String>, HostCatalogError> {
    Ok(connection
        .query_row(
            "SELECT revision FROM host_agent_pictures WHERE agent_id = ?1",
            [agent.to_string()],
            |row| row.get(0),
        )
        .optional()?)
}

/// Applies `change`, already validated, to `agent` in `transaction`, and
/// returns the revision now set. A retry of a recorded change returns what it
/// returned the first time.
pub(super) fn apply(
    transaction: &Transaction<'_>,
    agent: AgentId,
    change: &PictureChange,
    revision: Option<String>,
) -> Result<Option<String>, PictureError> {
    let request = Request {
        agent_id: agent.to_string(),
        expected_revision: change.expected_revision.clone(),
        revision: revision.clone(),
        content_type: change
            .picture
            .as_ref()
            .map(|picture| picture.content_type.clone()),
    };
    let receipt: Option<(String, String)> = transaction
        .query_row(
            "SELECT request_json, result_json FROM host_agent_picture_operations
             WHERE operation_id = ?1",
            [change.operation_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((stored, result)) = receipt {
        let stored: Request = serde_json::from_str(&stored)
            .map_err(|error| HostCatalogError::Invalid(error.to_string()))?;
        if stored != request {
            return Err(PictureError::OperationReused(change.operation_id));
        }
        return serde_json::from_str(&result)
            .map_err(|error| HostCatalogError::Invalid(error.to_string()).into());
    }
    let current = self::revision(transaction, agent)?;
    if current != change.expected_revision {
        return Err(PictureError::Stale { current });
    }
    match &change.picture {
        Some(picture) => transaction.execute(
            "INSERT INTO host_agent_pictures(agent_id, content_type, bytes, revision)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(agent_id) DO UPDATE SET
                content_type = excluded.content_type,
                bytes = excluded.bytes,
                revision = excluded.revision",
            params![
                agent.to_string(),
                picture.content_type,
                picture.bytes,
                revision
            ],
        )?,
        None => transaction.execute(
            "DELETE FROM host_agent_pictures WHERE agent_id = ?1",
            [agent.to_string()],
        )?,
    };
    let request = serde_json::to_string(&request)
        .map_err(|error| HostCatalogError::Invalid(error.to_string()))?;
    let result = serde_json::to_string(&revision)
        .map_err(|error| HostCatalogError::Invalid(error.to_string()))?;
    transaction.execute(
        "INSERT INTO host_agent_picture_operations(operation_id, agent_id, request_json, result_json)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            change.operation_id.to_string(),
            agent.to_string(),
            request,
            result
        ],
    )?;
    Ok(revision)
}

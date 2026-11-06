//! Files an agent shares with the person it talks to.
//!
//! `share_file` records a share: the agent, the person, the workspace the
//! turn ran in, and the file's path there. A share names a file by its path,
//! never by its bytes. Each read resolves the path again, so a share serves the
//! file as it is now, serves nothing while the file is gone, and refuses a path
//! that resolves outside its workspace when it is read. A surface reads a share through the owner's
//! Host calls: its description, then its bytes in [`UPLOAD_PART_BYTES`] parts,
//! the size that fits one RCP message, as uploads arrive.

use std::{
    io::{self, Read as _, Seek as _, SeekFrom},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use renoa_home::RenoaHome;
use renoa_kernel::AgentId;
use rusqlite::{OptionalExtension as _, Transaction, TransactionBehavior, params};
use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    host::catalog::{self, HostCatalogError},
    uploads::{MAX_UPLOAD_BYTES, UPLOAD_PART_BYTES},
    workspace::hex_sha256,
};

/// The largest file an agent may share: the most a person may upload, so a
/// file goes both ways under one limit.
pub(crate) const MAX_SHARE_BYTES: u64 = MAX_UPLOAD_BYTES;

/// The shares person `?2` may reach: their own, and those made in a turn with
/// no person, which belong to whoever owns the Host. Reading a share and
/// stopping one both use it, so a share that can be read can be stopped.
const REACHABLE: &str = "(principal_id IS NULL OR principal_id = ?2)";

/// [`UPLOAD_PART_BYTES`] as a file offset.
const PART: u64 = UPLOAD_PART_BYTES as u64;

/// Schema 53: the shares agents keep. One row per file an agent shares with
/// one person, or with whoever owns the Host when its turn had no person.
pub(crate) fn initialize(tx: &Transaction<'_>) -> Result<(), HostCatalogError> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_shares (
            share_id TEXT PRIMARY KEY CHECK (length(share_id) = 36),
            agent_id TEXT NOT NULL REFERENCES host_agents(agent_id) ON DELETE CASCADE,
            principal_id TEXT CHECK (principal_id IS NULL OR length(principal_id) = 36),
            workspace TEXT NOT NULL CHECK (length(workspace) > 0),
            path TEXT NOT NULL CHECK (length(path) > 0),
            created_at_ms INTEGER NOT NULL
        ) STRICT;
        CREATE UNIQUE INDEX IF NOT EXISTS host_shares_by_file
            ON host_shares(agent_id, ifnull(principal_id, ''), workspace, path);",
    )?;
    Ok(())
}

/// Turns a schema 53 catalog back into schema 52's tables.
#[cfg(test)]
pub(crate) fn restore_schema_52(connection: &rusqlite::Connection) {
    connection
        .execute_batch("DROP TABLE host_shares;")
        .expect("restore schema 52");
}

/// Where one file sits: a canonical workspace and a path inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SharedPath {
    pub(crate) workspace: PathBuf,
    pub(crate) path: String,
}

/// Records that `agent` shares `file` with `principal`, or with the Host's
/// owner when the turn had no person, and returns the share. Sharing the same
/// file again returns the same share.
pub(crate) fn share(
    database: &Path,
    agent: AgentId,
    principal: Option<Uuid>,
    file: &SharedPath,
) -> Result<Uuid, HostCatalogError> {
    let workspace = utf8(&file.workspace)?;
    let principal = principal.map(|principal| principal.to_string());
    let mut db = catalog::open_verified(database)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        "INSERT INTO host_shares(share_id, agent_id, principal_id, workspace, path, created_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT DO NOTHING",
        params![
            Uuid::new_v4().to_string(),
            agent.to_string(),
            principal,
            workspace,
            file.path,
            now_ms()
        ],
    )?;
    let id: String = tx.query_row(
        "SELECT share_id FROM host_shares
         WHERE agent_id = ?1 AND ifnull(principal_id, '') = ifnull(?2, '')
           AND workspace = ?3 AND path = ?4",
        params![agent.to_string(), principal, workspace, file.path],
        |row| row.get(0),
    )?;
    tx.commit()?;
    Uuid::parse_str(&id).map_err(|_| HostCatalogError::Invalid(format!("share id {id}")))
}

/// Stops `agent` sharing `file` with `principal`, including a share made in a
/// turn with no person; a turn with no person stops only those. Returns
/// whether anything was shared.
pub(crate) fn unshare(
    database: &Path,
    agent: AgentId,
    principal: Option<Uuid>,
    file: &SharedPath,
) -> Result<bool, HostCatalogError> {
    let db = catalog::open_verified(database)?;
    let removed = db.execute(
        &format!(
            "DELETE FROM host_shares
             WHERE agent_id = ?1 AND {REACHABLE} AND workspace = ?3 AND path = ?4"
        ),
        params![
            agent.to_string(),
            principal.map(|principal| principal.to_string()),
            utf8(&file.workspace)?,
            file.path
        ],
    )?;
    Ok(removed > 0)
}

fn utf8(path: &Path) -> Result<&str, HostCatalogError> {
    path.to_str()
        .ok_or_else(|| HostCatalogError::Invalid(format!("{} is not UTF-8", path.display())))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ShareError {
    /// No share has this id for this person, or its file is gone.
    #[error("Nothing is shared at this address any more.")]
    NotFound,
    #[error("This file is {size} bytes; a shared file is at most {MAX_SHARE_BYTES}.")]
    TooLarge { size: u64 },
    #[error("This file has {parts} parts; part {index} is past its end.")]
    NoSuchPart { index: u64, parts: u64 },
    #[error(transparent)]
    Catalog(#[from] HostCatalogError),
    #[error("shared file storage failed: {0}")]
    Io(#[from] io::Error),
    #[error("shared file task failed: {0}")]
    Background(#[from] tokio::task::JoinError),
}

/// What a surface needs before it reads a share's parts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShareDescription {
    /// The file's own name, without its folders.
    pub name: String,
    pub media_type: &'static str,
    pub size: u64,
    pub sha256: String,
    pub part_bytes: u64,
    pub parts: u64,
}

/// The shares of one Renoa home, as the owner's Host calls read them.
#[derive(Clone, Debug)]
pub struct Shares {
    database: PathBuf,
}

impl Shares {
    #[must_use]
    pub fn open(home: &RenoaHome) -> Self {
        Self {
            database: home.host_database(),
        }
    }

    /// Describes share `id` for `principal`. A share kept for another person,
    /// or whose file is gone, is [`ShareError::NotFound`].
    ///
    /// # Errors
    ///
    /// Returns not found, a file over [`MAX_SHARE_BYTES`], or a storage failure.
    pub async fn describe(
        &self,
        id: Uuid,
        principal: Uuid,
    ) -> Result<ShareDescription, ShareError> {
        let database = self.database.clone();
        tokio::task::spawn_blocking(move || {
            let (file, name) = find(&database, id, principal)?;
            let opened = open(&file)?;
            let size = opened.metadata()?.len();
            if size > MAX_SHARE_BYTES {
                return Err(ShareError::TooLarge { size });
            }
            let mut bytes = Vec::new();
            // A file that grew since it was measured is read only to the limit.
            opened.take(MAX_SHARE_BYTES + 1).read_to_end(&mut bytes)?;
            let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            if size > MAX_SHARE_BYTES {
                return Err(ShareError::TooLarge { size });
            }
            Ok(ShareDescription {
                media_type: media_type(&name),
                name,
                size,
                sha256: hex_sha256(&bytes),
                part_bytes: PART,
                parts: size.div_ceil(PART),
            })
        })
        .await?
    }

    /// Part `index` of share `id`, read from the file as it is now.
    ///
    /// # Errors
    ///
    /// Returns not found, a part past the file's end, a file over
    /// [`MAX_SHARE_BYTES`], or a storage failure.
    pub async fn part(&self, id: Uuid, principal: Uuid, index: u64) -> Result<Vec<u8>, ShareError> {
        let database = self.database.clone();
        tokio::task::spawn_blocking(move || {
            let (file, _) = find(&database, id, principal)?;
            let mut opened = open(&file)?;
            let size = opened.metadata()?.len();
            if size > MAX_SHARE_BYTES {
                return Err(ShareError::TooLarge { size });
            }
            let parts = size.div_ceil(PART);
            if index >= parts {
                return Err(ShareError::NoSuchPart { index, parts });
            }
            opened.seek(SeekFrom::Start(index * PART))?;
            let mut part = Vec::new();
            opened.take(PART).read_to_end(&mut part)?;
            Ok(part)
        })
        .await?
    }
}

/// The file share `id` names for `principal`, resolved again inside its
/// workspace, and its name.
fn find(database: &Path, id: Uuid, principal: Uuid) -> Result<(PathBuf, String), ShareError> {
    let db = catalog::open_read_only(database)?;
    let stored = db
        .query_row(
            &format!("SELECT workspace, path FROM host_shares WHERE share_id = ?1 AND {REACHABLE}"),
            params![id.to_string(), principal.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(HostCatalogError::from)?;
    let (workspace, path) = stored.ok_or(ShareError::NotFound)?;
    let workspace = PathBuf::from(workspace);
    let file = std::fs::canonicalize(workspace.join(&path)).map_err(unreachable)?;
    if !file.starts_with(&workspace) || !file.is_file() {
        return Err(ShareError::NotFound);
    }
    let name = file
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(ShareError::NotFound)?
        .to_owned();
    Ok((file, name))
}

fn open(file: &Path) -> Result<std::fs::File, ShareError> {
    std::fs::File::open(file).map_err(unreachable)
}

/// A file the Host can no longer reach by its path is not found: it is gone,
/// a folder on its way became a file, or it was closed to the Host. Asking
/// again cannot help, so none of these is a failure to retry.
fn unreachable(error: io::Error) -> ShareError {
    match error.kind() {
        io::ErrorKind::NotFound
        | io::ErrorKind::NotADirectory
        | io::ErrorKind::PermissionDenied => ShareError::NotFound,
        _ => error.into(),
    }
}

/// The media type a surface opens a file as, from its extension; anything
/// else is plain bytes for the phone to hand on.
fn media_type(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    match extension.as_deref() {
        Some("html" | "htm") => "text/html",
        Some("txt") => "text/plain",
        Some("md") => "text/markdown",
        Some("csv") => "text/csv",
        Some("json") => "application/json",
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests;

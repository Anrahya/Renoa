//! Static web sites an agent publishes from its workspace as immutable snapshots.
//!
//! Publishing validates and copies a workspace folder into `<state>/sites/<snapshot-id>`
//! before atomically recording the new snapshot in `host_sites`, then removes any
//! superseded snapshot. Later workspace edits do not change the live site, and a
//! failed publish leaves neither a partial directory nor a database row. The
//! node's site gateway serves the snapshots ([`Sites::entry`]).

mod snapshot;

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use renoa_home::RenoaHome;
use renoa_kernel::AgentId;
use rusqlite::{OptionalExtension as _, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::host::catalog::{self, HostCatalogError};
use snapshot::{
    CopyStats, DirCleanup, copy_verified_tree, create_private_dir, ensure_sites_dir, resolve_entry,
    resolve_source_folder,
};

pub use snapshot::SiteEntry;
pub(crate) use snapshot::{sites_dir_for_database, validate_site_name};

pub(crate) const MAX_SITE_FILES: usize = 512;
/// The most one site holds. Publishing copies the whole folder inside the
/// agent's turn and keeps it on the Host's disk until it is replaced, so this
/// bounds both; it fits a few minutes of 1080p clips.
pub(crate) const MAX_SITE_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
const REACHABLE: &str = "(principal_id IS NULL OR principal_id = ?1)";

/// Schema 55: published static site snapshots owned by this Host.
pub(crate) fn initialize(tx: &Transaction<'_>) -> Result<(), HostCatalogError> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_sites (
            site_name TEXT PRIMARY KEY CHECK (length(site_name) >= 1 AND length(site_name) <= 63),
            agent_id TEXT NOT NULL REFERENCES host_agents(agent_id) ON DELETE CASCADE,
            principal_id TEXT CHECK (principal_id IS NULL OR length(principal_id) = 36),
            snapshot_id TEXT NOT NULL UNIQUE CHECK (length(snapshot_id) = 36),
            source_folder TEXT NOT NULL CHECK (length(source_folder) > 0),
            file_count INTEGER NOT NULL CHECK (file_count > 0),
            total_bytes INTEGER NOT NULL CHECK (total_bytes >= 0),
            published_at_ms INTEGER NOT NULL
        ) STRICT;",
    )?;
    Ok(())
}

/// Turns a schema 55 catalog back into schema 54's tables.
#[cfg(test)]
pub(crate) fn restore_schema_54(connection: &rusqlite::Connection) {
    connection
        .execute_batch("DROP TABLE host_sites;")
        .expect("restore schema 54");
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SiteError {
    #[error("Nothing is published at this address.")]
    NotFound,
    #[error(
        "Site name must be 1 to 63 lowercase letters, numbers, or hyphens, starting and ending with a letter or number."
    )]
    InvalidName,
    #[error("This site name already belongs to another person.")]
    PermissionDenied,
    #[error("Site folder is invalid: {0}")]
    InvalidFolder(String),
    #[error("This folder has {file_count} files; a site is at most {MAX_SITE_FILES} files.")]
    TooManyFiles { file_count: usize },
    #[error("This site totals {total_bytes} bytes; a site is at most {MAX_SITE_TOTAL_BYTES}.")]
    SiteTooLarge { total_bytes: u64 },
    #[error(transparent)]
    Catalog(#[from] HostCatalogError),
    #[error("static site storage failed: {0}")]
    Io(#[from] io::Error),
    #[error("static site task failed: {0}")]
    Background(#[from] tokio::task::JoinError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedSite {
    pub name: String,
    pub agent_id: String,
    pub source_folder: String,
    pub file_count: u64,
    pub total_bytes: u64,
    pub published_at_ms: i64,
    pub path: String,
}

/// Validates and copies `folder` inside `workspace` to an immutable snapshot,
/// then records or atomically replaces `name` in `host_sites`.
pub(crate) fn publish(
    database: &Path,
    agent: AgentId,
    principal: Option<Uuid>,
    workspace: &Path,
    name: &str,
    folder: &str,
) -> Result<PublishedSite, SiteError> {
    let name = validate_site_name(name)?;
    let (source_path, normalized_folder) = resolve_source_folder(workspace, folder)?;
    let sites_dir = ensure_sites_dir(database)?;
    let snapshot_id = Uuid::new_v4().to_string();
    let staging = sites_dir.join(format!(".staging-{snapshot_id}"));
    create_private_dir(&staging)?;
    let mut staging_guard = DirCleanup::armed(staging.clone());

    let mut stats = CopyStats::default();
    copy_verified_tree(&source_path, &staging, &mut stats)?;
    if stats.file_count == 0 {
        return Err(SiteError::InvalidFolder(
            "the folder contains no files".to_owned(),
        ));
    }

    let snapshot_dir = sites_dir.join(&snapshot_id);
    fs::rename(&staging, &snapshot_dir)?;
    staging_guard.disarm();
    let mut snapshot_guard = DirCleanup::armed(snapshot_dir);

    let principal_text = principal.map(|id| id.to_string());
    let now = now_ms();
    let mut db = catalog::open_verified(database)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(HostCatalogError::from)?;
    let existing = tx
        .query_row(
            "SELECT principal_id, snapshot_id FROM host_sites WHERE site_name = ?1",
            [&name],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(HostCatalogError::from)?;
    if let Some((Some(owner), _)) = &existing
        && principal_text.as_deref() != Some(owner.as_str())
    {
        return Err(SiteError::PermissionDenied);
    }
    let old_snapshot = existing.map(|(_, id)| id);
    let file_count = u64::try_from(stats.file_count).unwrap_or(u64::MAX);
    tx.execute(
        "INSERT INTO host_sites (
            site_name, agent_id, principal_id, snapshot_id,
            source_folder, file_count, total_bytes, published_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(site_name) DO UPDATE SET
            agent_id = excluded.agent_id,
            principal_id = excluded.principal_id,
            snapshot_id = excluded.snapshot_id,
            source_folder = excluded.source_folder,
            file_count = excluded.file_count,
            total_bytes = excluded.total_bytes,
            published_at_ms = excluded.published_at_ms",
        params![
            name,
            agent.to_string(),
            principal_text,
            snapshot_id,
            normalized_folder,
            i64::try_from(file_count)
                .map_err(|_| HostCatalogError::Invalid("file_count overflow".into()))?,
            i64::try_from(stats.total_bytes)
                .map_err(|_| HostCatalogError::Invalid("total_bytes overflow".into()))?,
            now,
        ],
    )
    .map_err(HostCatalogError::from)?;
    tx.commit().map_err(HostCatalogError::from)?;
    snapshot_guard.disarm();

    if let Some(old_id) = old_snapshot
        && old_id != snapshot_id
    {
        let _ = fs::remove_dir_all(sites_dir.join(old_id));
    }

    Ok(PublishedSite {
        path: format!("/sites/{name}/"),
        name,
        agent_id: agent.to_string(),
        source_folder: normalized_folder,
        file_count,
        total_bytes: stats.total_bytes,
        published_at_ms: now,
    })
}

pub(crate) fn list_reachable(
    database: &Path,
    principal: Option<Uuid>,
) -> Result<Vec<PublishedSite>, SiteError> {
    let db = catalog::open_read_only(database)?;
    let sql = if principal.is_some() {
        format!(
            "SELECT site_name, agent_id, source_folder, file_count, total_bytes, published_at_ms
             FROM host_sites WHERE {REACHABLE} ORDER BY site_name ASC"
        )
    } else {
        "SELECT site_name, agent_id, source_folder, file_count, total_bytes, published_at_ms
         FROM host_sites WHERE principal_id IS NULL ORDER BY site_name ASC"
            .to_owned()
    };
    let mut stmt = db.prepare(&sql).map_err(HostCatalogError::from)?;
    let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<PublishedSite> {
        let name: String = row.get(0)?;
        Ok(PublishedSite {
            path: format!("/sites/{name}/"),
            name,
            agent_id: row.get(1)?,
            source_folder: row.get(2)?,
            file_count: u64::try_from(row.get::<_, i64>(3)?.max(0)).unwrap_or(0),
            total_bytes: u64::try_from(row.get::<_, i64>(4)?.max(0)).unwrap_or(0),
            published_at_ms: row.get(5)?,
        })
    };
    let rows = if let Some(principal) = principal {
        stmt.query_map([principal.to_string()], map_row)
            .map_err(HostCatalogError::from)?
            .collect::<Result<Vec<_>, _>>()
    } else {
        stmt.query_map([], map_row)
            .map_err(HostCatalogError::from)?
            .collect::<Result<Vec<_>, _>>()
    }
    .map_err(HostCatalogError::from)?;
    Ok(rows)
}

pub(crate) fn unpublish(
    database: &Path,
    sites_dir: &Path,
    principal: Option<Uuid>,
    name: &str,
) -> Result<bool, SiteError> {
    let name = validate_site_name(name)?;
    let mut db = catalog::open_verified(database)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(HostCatalogError::from)?;
    let snapshot_id: Option<String> = if let Some(principal) = principal {
        tx.query_row(
            &format!(
                "DELETE FROM host_sites WHERE site_name = ?2 AND {REACHABLE} RETURNING snapshot_id"
            ),
            params![principal.to_string(), name],
            |row| row.get(0),
        )
    } else {
        tx.query_row(
            "DELETE FROM host_sites WHERE site_name = ?1 AND principal_id IS NULL RETURNING snapshot_id",
            [name],
            |row| row.get(0),
        )
    }
    .optional()
    .map_err(HostCatalogError::from)?;
    tx.commit().map_err(HostCatalogError::from)?;
    if let Some(id) = snapshot_id {
        let _ = fs::remove_dir_all(sites_dir.join(id));
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Published static sites of one Renoa home, read by the owner's Host control.
#[derive(Clone, Debug)]
pub struct Sites {
    database: PathBuf,
    sites_dir: PathBuf,
}

impl Sites {
    #[must_use]
    pub fn open(home: &RenoaHome) -> Self {
        Self {
            database: home.host_database(),
            sites_dir: home.sites(),
        }
    }

    pub(crate) fn from_database(database: &Path) -> Self {
        Self {
            database: database.to_path_buf(),
            sites_dir: sites_dir_for_database(database),
        }
    }

    /// Publishes or atomically replaces static site `name` from `folder` in `workspace`.
    ///
    /// # Errors
    /// Returns validation, size, catalog, or storage failures.
    pub async fn publish(
        &self,
        agent: AgentId,
        principal: Option<Uuid>,
        workspace: PathBuf,
        name: String,
        folder: String,
    ) -> Result<PublishedSite, SiteError> {
        let database = self.database.clone();
        tokio::task::spawn_blocking(move || {
            publish(&database, agent, principal, &workspace, &name, &folder)
        })
        .await?
    }

    pub(crate) async fn list_for(
        &self,
        principal: Option<Uuid>,
    ) -> Result<Vec<PublishedSite>, SiteError> {
        let database = self.database.clone();
        tokio::task::spawn_blocking(move || list_reachable(&database, principal)).await?
    }

    pub(crate) async fn unpublish_for(
        &self,
        principal: Option<Uuid>,
        name: &str,
    ) -> Result<bool, SiteError> {
        let database = self.database.clone();
        let sites_dir = self.sites_dir.clone();
        let name = name.to_owned();
        tokio::task::spawn_blocking(move || unpublish(&database, &sites_dir, principal, &name))
            .await?
    }

    /// Lists every published site reachable by `principal`.
    ///
    /// # Errors
    /// Returns catalog failures.
    pub async fn list(&self, principal: Uuid) -> Result<Vec<PublishedSite>, SiteError> {
        self.list_for(Some(principal)).await
    }

    /// Removes a published site `name` reachable by `principal` and deletes its snapshot.
    ///
    /// # Errors
    /// Returns invalid name, catalog, or background task errors.
    pub async fn unpublish(&self, principal: Uuid, name: &str) -> Result<bool, SiteError> {
        self.unpublish_for(Some(principal), name).await
    }

    /// What `relative_path` names in published site `name`, for the owner's
    /// site gateway: every published site, whoever published it, since only
    /// the owner's own devices reach the gateway. The root without an
    /// `index.html` is a listing of the site's files.
    ///
    /// # Errors
    /// Returns [`SiteError::NotFound`] for an unknown site or path, or a
    /// catalog or storage failure.
    pub async fn entry(&self, name: &str, relative_path: &str) -> Result<SiteEntry, SiteError> {
        let database = self.database.clone();
        let sites_dir = self.sites_dir.clone();
        let name = name.to_owned();
        let relative_path = relative_path.to_owned();
        tokio::task::spawn_blocking(move || {
            resolve_entry(&database, &sites_dir, &name, &relative_path)
        })
        .await?
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests;

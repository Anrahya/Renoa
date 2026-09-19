//! The bounded clean-break reset of agent-owned Host state.
//!
//! A reset is explicit and repeatable. It applies the canonical schema cutover,
//! removes the rows and session directories that describe agents, and preserves
//! the Host's identity, catalogs, credentials, plugins, skill revisions, and
//! every workspace file on disk. Nothing here runs during ordinary startup: an
//! earlier data root fails closed until an operator applies this with a backup.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{LocalHostError, catalog};

/// Agent-owned tables, ordered so children are cleared before their parents.
const AGENT_OWNED_TABLES: &[&str] = &[
    "host_agent_tool_selections",
    "host_agent_mcp_connections",
    "host_agent_creations",
    "host_agent_tool_selection_operations",
    "host_agent_renames",
    "host_agents",
    "agent_skill_bindings",
    "agent_skill_source_rejections",
    "session_skills",
    "host_routine_deletions",
    "host_routine_mutations",
    "host_routine_owner_mutations",
    "host_routine_runs",
    "host_routines",
    "host_review_deliveries",
    "host_review_jobs",
    "host_review_operations",
    "host_review_publications",
    "host_review_requests",
    "host_review_runs",
    "host_review_repositories",
];

/// What one reset removed. Session directories and rows are counted separately
/// because they are separate stores; this never claims cross-store atomicity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostResetReport {
    /// Rows removed per table.
    pub removed_rows: BTreeMap<String, u64>,
    /// Session directories removed from the Host session root.
    pub removed_sessions: u64,
    /// Workspace directories left untouched, by name.
    pub preserved_workspaces: Vec<String>,
}

impl HostResetReport {
    /// The total number of rows one reset removed.
    #[must_use]
    pub fn total_rows(&self) -> u64 {
        self.removed_rows.values().copied().sum()
    }
}

/// Resets one Host data root: schema cutover, agent-owned rows, session
/// directories.
///
/// Applying it twice is safe. Workspace files under the data root are never
/// deleted, and every store outside the Host catalog is a separate step.
///
/// # Errors
/// Returns catalog storage or filesystem failures. A failed reset leaves the
/// database transaction uncommitted.
pub fn reset_host_data_root(data_directory: &Path) -> Result<HostResetReport, LocalHostError> {
    let database = data_directory.join(catalog::HOST_DATABASE);
    if database.exists() {
        catalog::cutover(&database)?;
    } else {
        std::fs::create_dir_all(data_directory)?;
        catalog::initialize(&database)?;
    }
    let removed_rows = clear_agent_rows(&database)?;
    let removed_sessions = clear_directory(&data_directory.join("sessions"))?
        + clear_directory(&data_directory.join("review-sessions"))?;
    Ok(HostResetReport {
        removed_rows,
        removed_sessions,
        preserved_workspaces: preserved_workspaces(data_directory),
    })
}

/// Removes every agent-owned row in one transaction.
fn clear_agent_rows(database: &Path) -> Result<BTreeMap<String, u64>, LocalHostError> {
    let mut connection = catalog::open_verified(database)?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(super::definition::catalog_error)?;
    let mut removed = BTreeMap::new();
    for table in AGENT_OWNED_TABLES {
        let deleted = transaction
            .execute(&format!("DELETE FROM {table}"), [])
            .map_err(super::definition::catalog_error)?;
        removed.insert((*table).to_owned(), deleted as u64);
    }
    transaction
        .commit()
        .map_err(super::definition::catalog_error)?;
    Ok(removed)
}

/// Removes each entry inside one directory, keeping the directory itself.
fn clear_directory(path: &Path) -> Result<u64, LocalHostError> {
    if !path.is_dir() {
        return Ok(0);
    }
    let mut removed = 0;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(&entry_path)?;
        } else {
            std::fs::remove_file(&entry_path)?;
        }
        removed += 1;
    }
    Ok(removed)
}

/// Names the workspace directories a reset leaves in place.
fn preserved_workspaces(data_directory: &Path) -> Vec<String> {
    let mut preserved = Vec::new();
    for name in ["agent-workspaces", "bot-workspaces"] {
        let path: PathBuf = data_directory.join(name);
        if path.is_dir() {
            preserved.push(name.to_owned());
        }
    }
    preserved
}

#[cfg(test)]
mod tests;

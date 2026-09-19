//! The bounded clean-break reset of agent-owned Host state.
//!
//! A reset is explicit and repeatable. It applies the canonical schema cutover,
//! removes the rows, session directories and document roots that describe
//! agents, and preserves the Host's identity, catalogs, credentials, plugins,
//! skill revisions, and every workspace file on disk. Nothing here runs during
//! ordinary startup: an earlier data root fails closed until an operator applies
//! this with a backup.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{LocalHostError, catalog};

/// Agent-owned tables, ordered so children precede their parents.
///
/// The order is for readers: the delete set is one transaction with deferred
/// foreign keys, so a table added here in the wrong place cannot break a reset.
/// Deferral only catches an omission that leaves a row referencing a deleted
/// parent, so completeness is enforced by
/// `every_catalog_table_is_classified_agent_owned_or_shared`, not by the commit.
const AGENT_OWNED_TABLES: &[&str] = &[
    "host_review_deliveries",
    "host_review_jobs",
    "host_review_operations",
    "host_review_publications",
    "host_review_runs",
    "host_review_requests",
    "host_review_repositories",
    "host_routine_deletions",
    "host_routine_runs",
    "host_routines",
    "host_routine_mutations",
    "host_routine_owner_mutations",
    "host_agent_tool_selections",
    "host_agent_mcp_connections",
    "host_agent_creations",
    "host_agent_tool_selection_operations",
    "host_agent_renames",
    "host_agents",
    "agent_skill_bindings",
    "agent_skill_source_rejections",
    "session_skills",
];

// Managed roots a reset clears, grouped by the report field that counts them.
// Every root named here is preflighted before the catalog is touched.
const SESSION_ROOTS: [&str; 2] = ["sessions", "review-sessions"];
const REVIEW_ROOTS: [&str; 2] = ["review-workspaces", "github-executions"];
const DOCUMENT_ROOTS: [&str; 2] = ["agents", "profiles"];

/// What one reset removed. Rows, session directories, review inspection
/// directories and document roots are counted separately because they are
/// separate stores; this never claims cross-store atomicity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostResetReport {
    /// Rows removed per table.
    pub removed_rows: BTreeMap<String, u64>,
    /// Session directories removed from the Host session root.
    pub removed_sessions: u64,
    /// Review inspection directories removed: the frozen checkouts and GitHub
    /// execution records keyed by the request ids the reset deletes, which
    /// nothing else can reap afterwards.
    pub removed_review_directories: u64,
    /// Agent document directories removed, under the canonical `agents/` root
    /// and the predecessor `profiles/` root alike.
    pub removed_document_roots: u64,
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
/// directories, and agent document roots.
///
/// Applying it twice is safe. Workspace files under the data root are never
/// deleted, and every store outside the Host catalog is a separate step.
///
/// # Errors
/// Returns catalog storage or filesystem failures. A reset that is refused —
/// an unusable managed root, or a catalog failure — leaves the database
/// untouched. A failure while removing directory contents can leave the
/// agent-owned rows deleted and the managed roots partly cleared; both removals
/// are idempotent, so re-running the reset converges.
pub fn reset_host_data_root(data_directory: &Path) -> Result<HostResetReport, LocalHostError> {
    // Every managed root is inspected before the catalog is touched, so a
    // refusal here leaves the rows and the state they describe in place.
    require_managed_roots(data_directory)?;
    let database = data_directory.join(catalog::HOST_DATABASE);
    if database.exists() {
        catalog::cutover(&database)?;
    } else {
        std::fs::create_dir_all(data_directory)?;
        catalog::initialize(&database)?;
    }
    let removed_rows = clear_agent_rows(&database)?;
    let removed_sessions = clear_roots(data_directory, &SESSION_ROOTS)?;
    let removed_review_directories = clear_roots(data_directory, &REVIEW_ROOTS)?;
    let removed_document_roots = clear_roots(data_directory, &DOCUMENT_ROOTS)?;
    Ok(HostResetReport {
        removed_rows,
        removed_sessions,
        removed_review_directories,
        removed_document_roots,
        preserved_workspaces: preserved_workspaces(data_directory),
    })
}

/// Removes every agent-owned row in one transaction.
fn clear_agent_rows(database: &Path) -> Result<BTreeMap<String, u64>, LocalHostError> {
    let mut connection = catalog::open_verified(database)?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(super::definition::catalog_error)?;
    // Deferring foreign keys makes the delete set one unit: a child row left
    // behind after its parent is deleted fails the commit.
    transaction
        .execute_batch("PRAGMA defer_foreign_keys = ON;")
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

/// Refuses every unusable managed root before any catalog work, so a refused
/// reset leaves the database untouched.
fn require_managed_roots(data_directory: &Path) -> Result<(), LocalHostError> {
    for roots in [SESSION_ROOTS, REVIEW_ROOTS, DOCUMENT_ROOTS] {
        for name in roots {
            require_managed_directory(&data_directory.join(name))?;
        }
    }
    Ok(())
}

/// Classifies one managed root without following a link: `true` when it is a
/// plain directory, `false` when it does not exist, and an error for a symbolic
/// link or any other file, which a reset refuses instead of following or
/// silently skipping. A reset can therefore never delete through a link out of
/// the data root.
fn require_managed_directory(path: &Path) -> Result<bool, LocalHostError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(source.into()),
    };
    if metadata.is_dir() {
        return Ok(true);
    }
    Err(LocalHostError::InvalidRequest(format!(
        "refusing to clear `{}`: a managed root must be a plain directory, never a symbolic link or another file",
        path.display()
    )))
}

/// Removes each entry inside one group of managed roots, keeping the
/// directories themselves.
fn clear_roots(data_directory: &Path, roots: &[&str]) -> Result<u64, LocalHostError> {
    let mut removed = 0;
    for name in roots {
        removed += clear_directory(&data_directory.join(name))?;
    }
    Ok(removed)
}

/// Removes each entry inside one directory, keeping the directory itself.
fn clear_directory(path: &Path) -> Result<u64, LocalHostError> {
    if !require_managed_directory(path)? {
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

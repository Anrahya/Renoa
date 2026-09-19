//! The bounded clean-break cutover of one existing Host catalog.
//!
//! The reset owns this step: it runs the earlier migration ladder for the
//! domains the Host still owns, drops every retired agent-owned table, and
//! recreates the canonical tables empty.

use std::path::Path;

use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior};

use super::{
    HostCatalogError, SCHEMA_VERSION, agents,
    migrations::{
        MIGRATE_V1_TO_V2, MIGRATE_V2_TO_V3, MIGRATE_V3_TO_V4, MIGRATE_V4_TO_V5, MIGRATE_V5_TO_V6,
        MIGRATE_V6_TO_V7, MIGRATE_V7_TO_V8, MIGRATE_V8_TO_V9, MIGRATE_V9_TO_V10,
        MIGRATE_V10_TO_V11, MIGRATE_V11_TO_V12, MIGRATE_V12_TO_V13,
    },
    open, verify,
};

/// Applies the canonical agent cutover to one existing Host catalog.
///
/// This is the bounded reset's schema step. It runs the earlier migration ladder
/// for the shared domains it still owns, drops every retired agent-owned table,
/// and recreates the canonical tables empty. Applying it to a catalog that is
/// already current only verifies it.
///
/// # Errors
///
/// Returns catalog storage failures or a catalog that cannot reach the current
/// schema.
pub(crate) fn cutover(path: &Path) -> Result<(), HostCatalogError> {
    let mut connection = open(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    migrate(&mut connection)
}

fn migrate(connection: &mut Connection) -> Result<(), HostCatalogError> {
    connection.pragma_update(None, "foreign_keys", false)?;
    let migration = (|| {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version =
            transaction.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?;
        match version {
            SCHEMA_VERSION => transaction.commit().map_err(HostCatalogError::from),
            version if (1..SCHEMA_VERSION).contains(&version) => {
                if version <= 2 {
                    require_complete_selected_catalogs(&transaction)?;
                }
                for (source_version, migration) in [
                    (1, MIGRATE_V1_TO_V2),
                    (2, MIGRATE_V2_TO_V3),
                    (3, MIGRATE_V3_TO_V4),
                    (4, MIGRATE_V4_TO_V5),
                    (5, MIGRATE_V5_TO_V6),
                    (6, MIGRATE_V6_TO_V7),
                    (7, MIGRATE_V7_TO_V8),
                    (8, MIGRATE_V8_TO_V9),
                    (9, MIGRATE_V9_TO_V10),
                    (10, MIGRATE_V10_TO_V11),
                    (11, MIGRATE_V11_TO_V12),
                    (12, MIGRATE_V12_TO_V13),
                ] {
                    if source_version >= version {
                        transaction.execute_batch(migration)?;
                    }
                }
                if version < 14 {
                    agents::initialize(&transaction)?;
                }
                if version < 26 {
                    retire_agent_owners(&transaction)?;
                }
                crate::host::definition::schema::initialize(&transaction)?;
                crate::host::routines::initialize(&transaction)?;
                crate::host::reviews::initialize(&transaction)?;
                crate::skills::SkillStore::initialize_tables(&transaction)?;
                transaction.execute(
                    "UPDATE host_metadata SET schema_version=?1 WHERE singleton=1",
                    [SCHEMA_VERSION],
                )?;
                transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
                transaction.commit()?;
                Ok(())
            }
            found => Err(HostCatalogError::Invalid(format!(
                "schema {found} is unsupported; expected {SCHEMA_VERSION}"
            ))),
        }
    })();
    let foreign_keys = connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(HostCatalogError::from);
    migration?;
    foreign_keys?;
    verify(connection)
}

fn require_complete_selected_catalogs(connection: &Connection) -> Result<(), HostCatalogError> {
    let missing = connection
        .query_row(
            "SELECT binding.connection_id
             FROM profile_mcp_tools AS binding
             LEFT JOIN mcp_catalogs AS catalog
               ON catalog.connection_id = binding.connection_id
             WHERE catalog.connection_id IS NULL
             LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(connection_id) = missing {
        return Err(HostCatalogError::Invalid(format!(
            "selected connection '{connection_id}' has no complete catalog"
        )));
    }
    Ok(())
}

/// Drops the agent-owned tables an earlier runtime owned, so the canonical
/// initializers below can recreate them in their current shape.
///
/// This is the schema half of the clean break: a data root written by an earlier
/// runtime keeps its Host identity, catalogs, credentials, plugins and skill
/// revisions, and loses the agent rows whose shape this runtime does not read.
fn retire_agent_owners(transaction: &rusqlite::Transaction<'_>) -> Result<(), HostCatalogError> {
    for table in [
        // Canonical owners whose shape or children changed.
        "host_agent_tool_selections",
        "host_agent_mcp_connections",
        "host_agent_creations",
        "host_agent_tool_selection_operations",
        "host_agent_renames",
        "host_agents",
        "agent_skill_bindings",
        "agent_skill_source_rejections",
        "session_skills",
        // Agent-owned records whose rows cannot survive the canonical shape.
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
        // Retired owners from earlier runtime versions.
        "host_bots",
        "host_bot_tool_selections",
        "host_bot_tool_operations",
        "host_bot_renames",
        "profile_mcp_connections",
        "profile_mcp_tools",
        "profile_skill_bindings",
        "skill_source_rejections",
    ] {
        transaction.execute_batch(&format!("DROP TABLE IF EXISTS {table};"))?;
    }
    Ok(())
}

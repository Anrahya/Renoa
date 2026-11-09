use std::{path::Path, time::Duration};

use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior};
use thiserror::Error;

mod agents;
mod automation_delivery;
mod automation_outcomes;
mod automation_rename;
mod base_schema;
mod cron_schedules;
mod cutover;
mod feed;
mod main_agent;
mod memory_dream;
mod memory_note_pass;
mod memory_plugin;
mod memory_reminders;
mod migrations;
mod owner_heartbeat;
mod owner_plugin;
mod registry_sync;
mod run_retention;
mod selection_migration;
mod time_plugin;

#[cfg(test)]
pub(crate) use automation_delivery::restore_schema_34_automations;
#[cfg(test)]
pub(crate) use automation_outcomes::restore_schema_36;
#[cfg(test)]
pub(crate) use automation_rename::restore_routine_tables;
#[cfg(test)]
pub(crate) use cron_schedules::restore_schema_37;
pub(crate) use cutover::cutover_and_clear;
#[cfg(test)]
pub(crate) use cutover::{cutover, fail_next_clear_before_commit};
#[cfg(test)]
pub(crate) use feed::restore_schema_45;
#[cfg(test)]
pub(crate) use main_agent::restore_schema_41;
#[cfg(test)]
pub(crate) use memory_dream::restore_schema_43;
#[cfg(test)]
pub(crate) use memory_note_pass::restore_schema_44;
#[cfg(test)]
pub(crate) use memory_plugin::restore_schema_42;
#[cfg(test)]
pub(crate) use memory_reminders::restore_schema_46;
#[cfg(test)]
pub(crate) use owner_heartbeat::restore_schema_49;
#[cfg(test)]
pub(crate) use owner_plugin::{restore_schema_47, restore_schema_48};
#[cfg(test)]
pub(crate) use registry_sync::restore_schema_35;
#[cfg(test)]
pub(crate) use run_retention::{restore_schema_38, restore_schema_39};
#[cfg(test)]
pub(crate) use time_plugin::restore_schema_40;

const SCHEMA_VERSION: u32 = 54;
pub(crate) use renoa_home::HOST_DATABASE_PATH as HOST_DATABASE;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HostCatalogError {
    #[error("Host catalog storage failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Host catalog database failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("invalid Host catalog: {0}")]
    Invalid(String),
}

impl crate::LocalHostError {
    /// Whether this failure is SQLite finding the database busy or locked:
    /// another writer held it past the busy timeout, so a later attempt may
    /// succeed. Every other failure would happen again.
    #[must_use]
    pub fn catalog_busy(&self) -> bool {
        let mut next: Option<&(dyn std::error::Error + 'static)> = Some(self);
        while let Some(error) = next {
            let code = error
                .downcast_ref::<rusqlite::Error>()
                .and_then(rusqlite::Error::sqlite_error_code)
                .or_else(|| {
                    error
                        .downcast_ref::<rusqlite::ffi::Error>()
                        .map(|error| error.code)
                });
            if matches!(
                code,
                Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
            ) {
                return true;
            }
            next = error.source();
        }
        false
    }
}

pub(crate) fn initialize(path: &Path) -> Result<(), HostCatalogError> {
    let mut connection = open(path, rusqlite::OpenFlags::default())?;
    restrict_database_permissions(path)?;
    initialize_connection(&mut connection)
}

pub(crate) fn open_verified(path: &Path) -> Result<Connection, HostCatalogError> {
    let connection = open(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    verify(&connection)?;
    Ok(connection)
}

pub(crate) fn open_read_only(path: &Path) -> Result<Connection, HostCatalogError> {
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    verify(&connection)?;
    Ok(connection)
}

fn open(path: &Path, flags: rusqlite::OpenFlags) -> Result<Connection, HostCatalogError> {
    let connection = Connection::open_with_flags(path, flags)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA synchronous = FULL;",
    )?;
    Ok(connection)
}

/// Runs every step from schema `version` to [`SCHEMA_VERSION`] in `transaction`,
/// and returns the removals to report once it commits.
fn upgrade(
    transaction: &rusqlite::Transaction<'_>,
    version: u32,
) -> Result<Vec<crate::host::automations::Removed>, HostCatalogError> {
    // Schema 46's feed tables come first: the schema 40 step purges deleted
    // automations, and a purge removes their feed items.
    if version < 46 {
        feed::add_feed(transaction)?;
    }
    // Schema 32 already holds exact plugin activations; only the earlier
    // schemas need their selections migrated or refused.
    if version < 32 {
        selection_migration::migrate(transaction)?;
        crate::plugins::activation::schema::initialize_lifecycle(transaction, true)?;
    }
    if version < 34 {
        cutover::retire_review_tables(transaction)?;
        automation_rename::rename_routines(transaction)?;
    }
    if version < 35 {
        automation_delivery::deliver_runs_through_tasks(transaction)?;
    }
    if version < 36 {
        registry_sync::record_sync_failures(transaction)?;
    }
    if version < 37 {
        automation_outcomes::record_run_outcomes(transaction)?;
    }
    if version < 38 {
        cron_schedules::adopt_cron_schedules(transaction)?;
    }
    let purged = if version < 40 {
        run_retention::bound_run_history(transaction)?
    } else {
        Vec::new()
    };
    if version < 41 {
        time_plugin::move_turn_timing_to_plugin(transaction)?;
    }
    if version < 42 {
        main_agent::mark_main_agent(transaction)?;
    }
    if version < 43 {
        memory_plugin::admit_memory_plugin(transaction)?;
    }
    if version < 44 {
        memory_dream::add_triggers_and_memory_jobs(transaction)?;
    }
    if version < 45 {
        memory_note_pass::add_note_passes(transaction)?;
    }
    if version < 47 {
        memory_reminders::add_reminders(transaction)?;
    }
    if version < 48 {
        owner_plugin::admit_owner_plugin(transaction)?;
    }
    if version < 49 {
        crate::host::feed::add_wakes(transaction)?;
    }
    if version < 50 {
        owner_heartbeat::admit_every_plugin_trigger(transaction)?;
    }
    if version < 51 {
        crate::host::automations::add_follow_ups(transaction)?;
    }
    if version < 52 {
        crate::host::plugin_setups::initialize(transaction)?;
    }
    if version < 53 {
        crate::shares::initialize(transaction)?;
    }
    if version < 54 {
        crate::host::appearance::upgrade(transaction)?;
    }
    Ok(purged)
}

fn initialize_connection(connection: &mut Connection) -> Result<(), HostCatalogError> {
    let observed =
        connection.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?;
    if (1..28).contains(&observed) {
        return Err(HostCatalogError::Invalid(format!(
            "this data root holds schema {observed}; the canonical agent cutover discards agent-owned state, so start it with `renoa-host <config.json> reset <backup-directory>`"
        )));
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version =
        transaction.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?;
    match version {
        SCHEMA_VERSION => {
            verify(&transaction)?;
            transaction.commit()?;
            Ok(())
        }
        28..=53 => {
            let metadata = transaction.query_row(
                "SELECT schema_version FROM host_metadata WHERE singleton = 1",
                [],
                |row| row.get::<_, u32>(0),
            )?;
            if metadata != version {
                return Err(HostCatalogError::Invalid(
                    "Host schema version and metadata disagree".to_owned(),
                ));
            }
            let purged = upgrade(&transaction, version)?;
            transaction.execute(
                "UPDATE host_metadata SET schema_version=?1 WHERE singleton=1",
                [SCHEMA_VERSION],
            )?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            verify(&transaction)?;
            transaction.commit()?;
            crate::host::automations::report_removed(purged);
            Ok(())
        }
        0 => {
            transaction.execute_batch(base_schema::SCHEMA)?;
            agents::initialize(&transaction)?;
            crate::skills::SkillStore::initialize_tables(&transaction)?;
            crate::plugins::activation::schema::initialize_lifecycle(&transaction, false)?;
            super::definition::schema::initialize(&transaction)?;
            super::automations::initialize(&transaction)?;
            super::plugin_setups::initialize(&transaction)?;
            crate::shares::initialize(&transaction)?;
            super::appearance::initialize(&transaction)?;
            memory_dream::add_triggers_and_memory_jobs(&transaction)?;
            memory_note_pass::add_note_passes(&transaction)?;
            feed::add_feed(&transaction)?;
            crate::host::feed::add_urgency(&transaction)?;
            crate::host::feed::add_wakes(&transaction)?;
            memory_reminders::add_reminders(&transaction)?;
            registry_sync::record_sync_failures(&transaction)?;
            transaction.execute(
                "UPDATE host_metadata SET schema_version=?1 WHERE singleton=1",
                [SCHEMA_VERSION],
            )?;
            transaction.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            verify(&transaction)?;
            transaction.commit()?;
            Ok(())
        }
        found => Err(HostCatalogError::Invalid(format!(
            "schema {found} is unsupported; expected {SCHEMA_VERSION}"
        ))),
    }
}

fn verify(connection: &Connection) -> Result<(), HostCatalogError> {
    let version =
        connection.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))?;
    let metadata = connection
        .query_row(
            "SELECT schema_version FROM host_metadata WHERE singleton = 1",
            [],
            |row| row.get::<_, u32>(0),
        )
        .optional()?;
    if version != SCHEMA_VERSION || metadata != Some(SCHEMA_VERSION) {
        return Err(HostCatalogError::Invalid(
            "metadata is missing or incompatible".to_owned(),
        ));
    }
    crate::plugins::activation::schema::verify_owners(connection)?;
    let violation = connection
        .query_row("PRAGMA foreign_key_check", [], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(2)?))
        })
        .optional()?;
    if let Some((table, parent)) = violation {
        return Err(HostCatalogError::Invalid(format!(
            "foreign-key validation failed: {table} references {parent}"
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_database_permissions(path: &Path) -> Result<(), HostCatalogError> {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_database_permissions(_path: &Path) -> Result<(), HostCatalogError> {
    Ok(())
}

#[cfg(test)]
mod tests;

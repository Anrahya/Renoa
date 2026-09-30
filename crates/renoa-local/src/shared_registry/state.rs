use std::path::PathBuf;

use renoa_registry_protocol::RegistryId;
use rusqlite::{OptionalExtension as _, TransactionBehavior};

use super::SharedRegistryError;

#[derive(Clone)]
pub(super) struct RegistryState {
    database: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Cursor {
    pub(super) registry_id: RegistryId,
    pub(super) revision: u64,
}

impl RegistryState {
    pub(super) fn new(database: PathBuf) -> Self {
        Self { database }
    }

    pub(super) fn bind(&self, registry_id: RegistryId) -> Result<Cursor, SharedRegistryError> {
        let mut connection = crate::host::catalog::open_verified(&self.database)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = read_cursor(&transaction)?;
        let cursor = match stored {
            Some(cursor) if cursor.registry_id == registry_id => cursor,
            Some(cursor) => {
                return Err(SharedRegistryError::Conflict(format!(
                    "this Host is bound to shared registry {}, not {registry_id}",
                    cursor.registry_id
                )));
            }
            None => {
                transaction.execute(
                    "INSERT INTO shared_plugin_registry_state(
                        singleton, registry_id, applied_revision
                     ) VALUES (1, ?1, 0)",
                    [registry_id.to_string()],
                )?;
                Cursor {
                    registry_id,
                    revision: 0,
                }
            }
        };
        transaction.commit()?;
        Ok(cursor)
    }

    pub(super) fn advance(
        &self,
        registry_id: RegistryId,
        revision: u64,
    ) -> Result<Cursor, SharedRegistryError> {
        let stored_revision = sql_i64(revision)?;
        let mut connection = crate::host::catalog::open_verified(&self.database)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cursor = read_cursor(&transaction)?.ok_or_else(|| {
            SharedRegistryError::Conflict("shared registry identity was not bound".to_owned())
        })?;
        if cursor.registry_id != registry_id {
            return Err(SharedRegistryError::Conflict(format!(
                "this Host is bound to shared registry {}, not {registry_id}",
                cursor.registry_id
            )));
        }
        if revision > cursor.revision && revision != cursor.revision + 1 {
            return Err(SharedRegistryError::Protocol(format!(
                "shared registry revision jumped from {} to {revision}",
                cursor.revision
            )));
        }
        let revision = cursor.revision.max(revision);
        transaction.execute(
            "UPDATE shared_plugin_registry_state SET applied_revision = ?1
             WHERE singleton = 1",
            [stored_revision.max(sql_i64(cursor.revision)?)],
        )?;
        transaction.commit()?;
        Ok(Cursor {
            registry_id,
            revision,
        })
    }

    /// Records that the latest synchronization failed with `error`. Returns
    /// whether this began a failure or changed its reason; a repeated failure
    /// keeps the time it began and writes nothing.
    pub(super) fn record_failure(
        &self,
        error: &str,
        now_ms: i64,
    ) -> Result<bool, SharedRegistryError> {
        let mut connection = crate::host::catalog::open_verified(&self.database)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let recorded = transaction
            .query_row(
                "SELECT error FROM shared_plugin_registry_sync WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let changed = match recorded {
            Some(recorded) if recorded == error => false,
            Some(_) => {
                transaction.execute(
                    "UPDATE shared_plugin_registry_sync SET error = ?1 WHERE singleton = 1",
                    [error],
                )?;
                true
            }
            None => {
                transaction.execute(
                    "INSERT INTO shared_plugin_registry_sync(singleton, failing_since_ms, error)
                     VALUES (1, ?1, ?2)",
                    rusqlite::params![now_ms, error],
                )?;
                true
            }
        };
        transaction.commit()?;
        Ok(changed)
    }

    /// Clears the failure record after a synchronization succeeds. Returns
    /// whether one was recorded; a Host that was not failing writes nothing.
    pub(super) fn clear_failure(&self) -> Result<bool, SharedRegistryError> {
        let connection = crate::host::catalog::open_verified(&self.database)?;
        let failing = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM shared_plugin_registry_sync)",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if !failing {
            return Ok(false);
        }
        connection.execute("DELETE FROM shared_plugin_registry_sync", [])?;
        Ok(true)
    }
}

fn read_cursor(connection: &rusqlite::Connection) -> Result<Option<Cursor>, SharedRegistryError> {
    let stored = connection
        .query_row(
            "SELECT registry_id, applied_revision
             FROM shared_plugin_registry_state WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    stored
        .map(|(registry_id, revision)| {
            Ok(Cursor {
                registry_id: registry_id.parse().map_err(|error| {
                    SharedRegistryError::Protocol(format!(
                        "stored shared registry identity is invalid: {error}"
                    ))
                })?,
                revision: u64::try_from(revision).map_err(|_| {
                    SharedRegistryError::Protocol(
                        "stored shared registry revision is negative".to_owned(),
                    )
                })?,
            })
        })
        .transpose()
}

fn sql_i64(value: u64) -> Result<i64, SharedRegistryError> {
    i64::try_from(value).map_err(|_| {
        SharedRegistryError::Protocol("shared registry revision exceeds SQLite range".to_owned())
    })
}

#[cfg(test)]
mod tests {
    use rusqlite::OptionalExtension as _;

    use super::RegistryState;

    #[test]
    fn cursor_binds_once_and_advances_contiguously() {
        let directory = tempfile::tempdir().expect("temporary Host state");
        let database = directory.path().join("host.sqlite3");
        crate::host::catalog::initialize(&database).expect("initialize Host catalog");
        let state = RegistryState::new(database);
        let registry = renoa_registry_protocol::RegistryId::new();
        assert_eq!(state.bind(registry).expect("bind").revision, 0);
        assert_eq!(state.advance(registry, 1).expect("advance").revision, 1);
        assert_eq!(state.advance(registry, 1).expect("repeat").revision, 1);
        assert!(state.advance(registry, 3).is_err());
        assert!(
            state
                .bind(renoa_registry_protocol::RegistryId::new())
                .is_err()
        );
    }

    #[test]
    fn a_failure_is_recorded_once_per_reason_and_cleared_by_success() {
        let directory = tempfile::tempdir().expect("temporary Host state");
        let database = directory.path().join("host.sqlite3");
        crate::host::catalog::initialize(&database).expect("initialize Host catalog");
        let state = RegistryState::new(database.clone());
        let failure = |connection: &rusqlite::Connection| {
            connection
                .query_row(
                    "SELECT failing_since_ms, error FROM shared_plugin_registry_sync",
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .expect("read failure record")
        };
        let connection = rusqlite::Connection::open(&database).expect("open catalog");

        assert!(!state.clear_failure().expect("clear nothing"));
        assert!(state.record_failure("unreachable", 10).expect("record"));
        assert!(!state.record_failure("unreachable", 20).expect("repeat"));
        assert_eq!(failure(&connection), Some((10, "unreachable".to_owned())));
        assert!(state.record_failure("bound elsewhere", 30).expect("change"));
        assert_eq!(
            failure(&connection),
            Some((10, "bound elsewhere".to_owned())),
            "a changed reason keeps the time the failure began"
        );
        assert!(state.clear_failure().expect("clear"));
        assert_eq!(failure(&connection), None);
    }
}

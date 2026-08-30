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
}

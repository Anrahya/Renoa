use std::path::Path;

use rusqlite::{Connection, OptionalExtension};

use super::NodeStoreError;

/// The ledger shape this runtime reads.
///
/// Version 2 renamed the task's `profile_id` column to `agent_id`, so a ledger
/// written by an earlier runtime is refused by name instead of failing later on
/// a missing column.
const SCHEMA_VERSION: u32 = 2;

pub(super) fn initialize(path: &Path) -> Result<(), NodeStoreError> {
    let connection = open_connection(path)?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_node_metadata (
            singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
            schema_version INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS host_node_tasks (
            task_id TEXT PRIMARY KEY,
            target TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            session_id TEXT NOT NULL UNIQUE,
            workspace TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS host_node_executions (
            admission_sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            command_id TEXT NOT NULL UNIQUE,
            task_id TEXT NOT NULL REFERENCES host_node_tasks(task_id),
            command_json TEXT NOT NULL,
            execution_id TEXT NOT NULL UNIQUE,
            admission_acked INTEGER NOT NULL DEFAULT 0
                CHECK(admission_acked IN (0, 1)),
            terminal INTEGER NOT NULL DEFAULT 0
                CHECK(terminal IN (0, 1)),
            published_through INTEGER
                CHECK(published_through IS NULL OR published_through >= 0)
         );
         CREATE TABLE IF NOT EXISTS host_node_events (
            command_id TEXT NOT NULL REFERENCES host_node_executions(command_id),
            sequence INTEGER NOT NULL CHECK(sequence >= 0),
            event_json TEXT NOT NULL,
            PRIMARY KEY(command_id, sequence)
         );",
    )?;
    initialize_version(&connection)?;
    restrict_file_permissions(path)?;
    Ok(())
}

fn initialize_version(connection: &Connection) -> Result<(), NodeStoreError> {
    let stored = connection
        .query_row(
            "SELECT schema_version FROM host_node_metadata WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    match stored {
        Some(version) if version == i64::from(SCHEMA_VERSION) => Ok(()),
        Some(version) => Err(NodeStoreError::Invalid(format!(
            "unsupported node ledger schema {version}; expected {SCHEMA_VERSION}"
        ))),
        None => {
            connection.execute(
                "INSERT INTO host_node_metadata (singleton, schema_version) VALUES (1, ?1)",
                [SCHEMA_VERSION],
            )?;
            Ok(())
        }
    }
}

pub(super) fn open_connection(path: &Path) -> Result<Connection, NodeStoreError> {
    let connection = Connection::open(path)?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = FULL;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(connection)
}

#[cfg(unix)]
fn restrict_file_permissions(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_file_permissions(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::initialize;
    use rusqlite::Connection;
    use tempfile::tempdir;

    #[test]
    fn an_earlier_ledger_shape_is_refused_by_name() {
        let directory = tempdir().expect("fixture");
        let path = directory.path().join("node.sqlite");
        initialize(&path).expect("current ledger");
        {
            let connection = Connection::open(&path).expect("open ledger");
            connection
                .execute_batch(
                    "DROP TABLE host_node_tasks;
                     CREATE TABLE host_node_tasks (
                        task_id TEXT PRIMARY KEY,
                        target TEXT NOT NULL,
                        profile_id TEXT NOT NULL,
                        session_id TEXT NOT NULL UNIQUE,
                        workspace TEXT NOT NULL
                     );
                     UPDATE host_node_metadata SET schema_version = 1;",
                )
                .expect("earlier ledger shape");
        }
        let error = initialize(&path).expect_err("an earlier ledger is refused");
        assert!(
            error
                .to_string()
                .contains("unsupported node ledger schema 1"),
            "{error}"
        );
    }
}

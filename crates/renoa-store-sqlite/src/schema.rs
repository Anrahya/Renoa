use renoa_core::{CommandEnvelope, StoreError};
use rusqlite::{Connection, TransactionBehavior, params};

use crate::{json_error, sqlite_error};

pub(crate) fn initialize(connection: &mut Connection) -> Result<(), StoreError> {
    connection
        .execute_batch(
            "
            CREATE TABLE IF NOT EXISTS runs (
                run_id TEXT PRIMARY KEY,
                command_id TEXT NOT NULL,
                command_json TEXT NOT NULL,
                agent_json TEXT NOT NULL,
                status TEXT NOT NULL,
                terminal_json TEXT,
                next_sequence INTEGER NOT NULL,
                created_at_ms INTEGER NOT NULL,
                finished_at_ms INTEGER
            );

            CREATE TABLE IF NOT EXISTS run_events (
                event_id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL REFERENCES runs(run_id),
                sequence INTEGER NOT NULL,
                recorded_at_ms INTEGER NOT NULL,
                kind_json TEXT NOT NULL,
                UNIQUE(run_id, sequence)
            );

            CREATE INDEX IF NOT EXISTS run_events_run_sequence
                ON run_events(run_id, sequence);
            ",
        )
        .map_err(sqlite_error)?;
    migrate_command_identity(connection)
}

fn migrate_command_identity(connection: &mut Connection) -> Result<(), StoreError> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite_error)?;
    let has_command_id = {
        let mut statement = transaction
            .prepare("PRAGMA table_info(runs)")
            .map_err(sqlite_error)?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(sqlite_error)?;
        let mut found = false;
        for column in columns {
            found |= column.map_err(sqlite_error)? == "command_id";
        }
        found
    };

    if !has_command_id {
        transaction
            .execute("ALTER TABLE runs ADD COLUMN command_id TEXT", [])
            .map_err(sqlite_error)?;
        let legacy_runs = {
            let mut statement = transaction
                .prepare("SELECT run_id, command_json FROM runs")
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(sqlite_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(sqlite_error)?
        };
        for (run_id, command_json) in legacy_runs {
            let command: CommandEnvelope =
                serde_json::from_str(&command_json).map_err(json_error)?;
            transaction
                .execute(
                    "UPDATE runs SET command_id = ?2 WHERE run_id = ?1",
                    params![run_id, command.command_id.to_string()],
                )
                .map_err(sqlite_error)?;
        }
    }

    transaction
        .execute(
            "CREATE UNIQUE INDEX IF NOT EXISTS runs_command_id_unique
             ON runs(command_id)",
            [],
        )
        .map_err(sqlite_error)?;
    transaction.commit().map_err(sqlite_error)
}

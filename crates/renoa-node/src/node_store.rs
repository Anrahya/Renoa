use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use renoa_control::TaskId;
use renoa_core::{CommandId, RunId, StoreError};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExecutionRecord {
    pub(crate) task_id: TaskId,
    pub(crate) command_id: CommandId,
    pub(crate) run_id: RunId,
    pub(crate) admission_acked: bool,
    pub(crate) published_through: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) struct NodeStore {
    path: Arc<PathBuf>,
}

impl NodeStore {
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        let connection = open_connection(&path)?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS node_executions (
                    command_id TEXT PRIMARY KEY,
                    task_id TEXT NOT NULL,
                    run_id TEXT NOT NULL UNIQUE REFERENCES runs(run_id),
                    admission_acked INTEGER NOT NULL DEFAULT 0
                        CHECK(admission_acked IN (0, 1)),
                    published_through INTEGER
                        CHECK(published_through IS NULL OR published_through >= 0)
                );",
            )
            .map_err(sqlite_error)?;
        Ok(Self {
            path: Arc::new(path),
        })
    }

    pub(crate) async fn admit(
        &self,
        task_id: TaskId,
        command_id: CommandId,
        run_id: RunId,
    ) -> Result<ExecutionRecord, StoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            if let Some(existing) = load_record(&transaction, command_id)? {
                if existing.task_id != task_id || existing.run_id != run_id {
                    return Err(StoreError::new(format!(
                        "command {command_id} already maps to a different execution"
                    )));
                }
                return Ok(existing);
            }
            transaction
                .execute(
                    "INSERT INTO node_executions (
                        command_id, task_id, run_id, admission_acked, published_through
                     ) VALUES (?1, ?2, ?3, 0, NULL)",
                    params![
                        command_id.to_string(),
                        task_id.to_string(),
                        run_id.to_string()
                    ],
                )
                .map_err(sqlite_error)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(ExecutionRecord {
                task_id,
                command_id,
                run_id,
                admission_acked: false,
                published_through: None,
            })
        })
        .await
    }

    pub(crate) async fn load_all(&self) -> Result<Vec<ExecutionRecord>, StoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let mut statement = connection
                .prepare(
                    "SELECT task_id, command_id, run_id, admission_acked, published_through
                     FROM node_executions ORDER BY command_id",
                )
                .map_err(sqlite_error)?;
            let rows = statement.query_map([], stored_row).map_err(sqlite_error)?;
            rows.map(|row| {
                row.map_err(sqlite_error)
                    .and_then(|record| decode_record(&record))
            })
            .collect()
        })
        .await
    }

    pub(crate) async fn find(
        &self,
        command_id: CommandId,
    ) -> Result<Option<ExecutionRecord>, StoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            load_record(&connection, command_id)
        })
        .await
    }

    pub(crate) async fn require_admission_ack(
        &self,
        command_id: CommandId,
    ) -> Result<(), StoreError> {
        self.set_admission_ack(command_id, false).await
    }

    pub(crate) async fn acknowledge_admission(
        &self,
        command_id: CommandId,
    ) -> Result<(), StoreError> {
        self.set_admission_ack(command_id, true).await
    }

    pub(crate) async fn advance_publication(
        &self,
        command_id: CommandId,
        through_sequence: u64,
    ) -> Result<(), StoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let through_sequence = to_sql_integer(through_sequence)?;
            let changed = connection
                .execute(
                    "UPDATE node_executions
                     SET published_through = CASE
                         WHEN published_through IS NULL OR published_through < ?2 THEN ?2
                         ELSE published_through
                     END
                     WHERE command_id = ?1",
                    params![command_id.to_string(), through_sequence],
                )
                .map_err(sqlite_error)?;
            require_one(changed, command_id)
        })
        .await
    }

    async fn set_admission_ack(
        &self,
        command_id: CommandId,
        acknowledged: bool,
    ) -> Result<(), StoreError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let changed = connection
                .execute(
                    "UPDATE node_executions SET admission_acked = ?2 WHERE command_id = ?1",
                    params![command_id.to_string(), acknowledged],
                )
                .map_err(sqlite_error)?;
            require_one(changed, command_id)
        })
        .await
    }
}

fn load_record(
    connection: &Connection,
    command_id: CommandId,
) -> Result<Option<ExecutionRecord>, StoreError> {
    let stored = connection
        .query_row(
            "SELECT task_id, command_id, run_id, admission_acked, published_through
             FROM node_executions WHERE command_id = ?1",
            [command_id.to_string()],
            stored_row,
        )
        .optional()
        .map_err(sqlite_error)?;
    stored.as_ref().map(decode_record).transpose()
}

fn stored_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(String, String, String, bool, Option<i64>)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}

fn decode_record(
    row: &(String, String, String, bool, Option<i64>),
) -> Result<ExecutionRecord, StoreError> {
    Ok(ExecutionRecord {
        task_id: TaskId::from_uuid(parse_id(&row.0, "task")?),
        command_id: CommandId::from_uuid(parse_id(&row.1, "command")?),
        run_id: RunId::from_uuid(parse_id(&row.2, "run")?),
        admission_acked: row.3,
        published_through: row
            .4
            .map(|value| {
                u64::try_from(value).map_err(|_| StoreError::new("negative publication cursor"))
            })
            .transpose()?,
    })
}

fn open_connection(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open(path).map_err(sqlite_error)?;
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(sqlite_error)?;
    Ok(connection)
}

async fn blocking<T>(
    operation: impl FnOnce() -> Result<T, StoreError> + Send + 'static,
) -> Result<T, StoreError>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| StoreError::new(format!("node SQLite worker failed: {error}")))?
}

fn require_one(changed: usize, command_id: CommandId) -> Result<(), StoreError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(StoreError::new(format!(
            "execution for command {command_id} was not found"
        )))
    }
}

fn parse_id(value: &str, name: &str) -> Result<Uuid, StoreError> {
    value
        .parse()
        .map_err(|error| StoreError::new(format!("invalid {name} id: {error}")))
}

fn to_sql_integer(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::new("integer exceeds SQLite i64 range"))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the owned signature is required by Result::map_err"
)]
fn sqlite_error(error: rusqlite::Error) -> StoreError {
    StoreError::new(format!("node SQLite error: {error}"))
}

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use renoa_core::{
    BoxFuture, CommandEnvelope, EventId, ResolvedAgent, RunAdmission, RunEvent, RunEventKind,
    RunId, RunRecord, RunStatus, RunStore, RunTranscript, StoreError, TerminalState,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

mod schema;

#[derive(Debug, Clone)]
pub struct SqliteRunStore {
    path: Arc<PathBuf>,
}

impl SqliteRunStore {
    /// Opens or creates a run ledger at `path` and applies its schema.
    ///
    /// # Errors
    ///
    /// Returns `StoreError` when `SQLite` cannot open or initialize the ledger.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        let mut connection = open_connection(&path)?;
        schema::initialize(&mut connection)?;
        Ok(Self {
            path: Arc::new(path),
        })
    }

    fn path(&self) -> Arc<PathBuf> {
        Arc::clone(&self.path)
    }

    /// Loads the durable events after an optional acknowledged source cursor.
    ///
    /// # Errors
    ///
    /// Returns `StoreError` when the run does not exist or its ledger cannot be
    /// read.
    pub async fn load_events_after(
        &self,
        run_id: RunId,
        after_sequence: Option<u64>,
    ) -> Result<Vec<RunEvent>, StoreError> {
        let path = self.path();
        tokio::task::spawn_blocking(move || {
            let connection = open_connection(&path)?;
            let exists = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM runs WHERE run_id = ?1)",
                    [run_id.to_string()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(sqlite_error)?;
            if !exists {
                return Err(StoreError::new(format!("run {run_id} was not found")));
            }
            let after_sequence = after_sequence.map(to_sql_integer).transpose()?;
            let mut statement = connection
                .prepare(
                    "SELECT event_id, sequence, recorded_at_ms, kind_json
                     FROM run_events
                     WHERE run_id = ?1 AND (?2 IS NULL OR sequence > ?2)
                     ORDER BY sequence",
                )
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map(params![run_id.to_string(), after_sequence], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(sqlite_error)?;
            let mut events = Vec::new();
            for row in rows {
                let (event_id, sequence, recorded_at_ms, kind_json) = row.map_err(sqlite_error)?;
                events.push(RunEvent {
                    event_id: EventId::from_uuid(
                        event_id.parse().map_err(|error| {
                            StoreError::new(format!("invalid event id: {error}"))
                        })?,
                    ),
                    run_id,
                    sequence: u64::try_from(sequence)
                        .map_err(|_| StoreError::new("negative event sequence"))?,
                    recorded_at_ms,
                    kind: serde_json::from_str(&kind_json).map_err(json_error)?,
                });
            }
            Ok(events)
        })
        .await
        .map_err(join_error)?
    }
}

impl RunStore for SqliteRunStore {
    fn admit_run(
        &self,
        command: CommandEnvelope,
        agent: ResolvedAgent,
    ) -> BoxFuture<'_, Result<RunAdmission, StoreError>> {
        let path = self.path();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let mut connection = open_connection(&path)?;
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(sqlite_error)?;
                let command_json = serde_json::to_string(&command).map_err(json_error)?;
                let agent_json = serde_json::to_string(&agent).map_err(json_error)?;
                let command_id = command.command_id.to_string();
                let existing = transaction
                    .query_row(
                        "SELECT run_id, command_json, agent_json
                         FROM runs WHERE command_id = ?1",
                        [&command_id],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(sqlite_error)?;
                if let Some((run_id, stored_command, stored_agent)) = existing {
                    let run_id =
                        RunId::from_uuid(run_id.parse().map_err(|error| {
                            StoreError::new(format!("invalid run id: {error}"))
                        })?);
                    let admission = if stored_command == command_json && stored_agent == agent_json
                    {
                        RunAdmission::Existing(run_id)
                    } else {
                        RunAdmission::Conflict(run_id)
                    };
                    return Ok(admission);
                }

                let run_id = RunId::new();
                let created_at_ms = now_ms()?;
                transaction
                    .execute(
                        "INSERT INTO runs (
                            run_id, command_id, command_json, agent_json, status,
                            terminal_json, next_sequence, created_at_ms, finished_at_ms
                         ) VALUES (?1, ?2, ?3, ?4, 'open', NULL, 1, ?5, NULL)",
                        params![
                            run_id.to_string(),
                            command_id,
                            command_json,
                            agent_json,
                            created_at_ms
                        ],
                    )
                    .map_err(sqlite_error)?;
                insert_event(
                    &transaction,
                    run_id,
                    0,
                    created_at_ms,
                    &RunEventKind::RunStarted {
                        command: command.clone(),
                        agent,
                    },
                )?;
                transaction.commit().map_err(sqlite_error)?;
                Ok(RunAdmission::Admitted(run_id))
            })
            .await
            .map_err(join_error)?
        })
    }

    fn append_events(
        &self,
        run_id: RunId,
        events: Vec<RunEventKind>,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        let path = self.path();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                if events.is_empty() {
                    return Ok(());
                }
                let mut connection = open_connection(&path)?;
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(sqlite_error)?;
                let (status, next_sequence) = load_open_cursor(&transaction, run_id)?;
                if status != RunStatus::Open {
                    return Err(StoreError::new(format!(
                        "cannot append events to terminal run {run_id}"
                    )));
                }
                let recorded_at_ms = now_ms()?;
                let event_count = events.len();
                for (offset, kind) in events.into_iter().enumerate() {
                    let sequence = next_sequence
                        + u64::try_from(offset).expect("event batch length must fit in u64");
                    insert_event(&transaction, run_id, sequence, recorded_at_ms, &kind)?;
                }
                let next_sequence = next_sequence
                    + u64::try_from(event_count).expect("event batch length must fit in u64");
                transaction
                    .execute(
                        "UPDATE runs SET next_sequence = ?2 WHERE run_id = ?1",
                        params![run_id.to_string(), to_sql_integer(next_sequence)?],
                    )
                    .map_err(sqlite_error)?;
                transaction.commit().map_err(sqlite_error)?;
                Ok(())
            })
            .await
            .map_err(join_error)?
        })
    }

    fn finish_run(
        &self,
        run_id: RunId,
        terminal: TerminalState,
    ) -> BoxFuture<'_, Result<(), StoreError>> {
        let path = self.path();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let mut connection = open_connection(&path)?;
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .map_err(sqlite_error)?;
                let (status, next_sequence) = load_open_cursor(&transaction, run_id)?;
                if status != RunStatus::Open {
                    return Err(StoreError::new(format!(
                        "run {run_id} already reached terminal state"
                    )));
                }
                let finished_at_ms = now_ms()?;
                let terminal_json = serde_json::to_string(&terminal).map_err(json_error)?;
                let changed = transaction
                    .execute(
                        "UPDATE runs
                         SET status = ?2, terminal_json = ?3, next_sequence = ?4,
                             finished_at_ms = ?5
                         WHERE run_id = ?1 AND status = 'open'",
                        params![
                            run_id.to_string(),
                            status_name(terminal.status()),
                            terminal_json,
                            to_sql_integer(next_sequence + 1)?,
                            finished_at_ms,
                        ],
                    )
                    .map_err(sqlite_error)?;
                if changed != 1 {
                    return Err(StoreError::new(format!(
                        "terminal compare-and-set lost for run {run_id}"
                    )));
                }
                insert_event(
                    &transaction,
                    run_id,
                    next_sequence,
                    finished_at_ms,
                    &RunEventKind::RunTerminated {
                        terminal: terminal.clone(),
                    },
                )?;
                transaction.commit().map_err(sqlite_error)?;
                Ok(())
            })
            .await
            .map_err(join_error)?
        })
    }

    fn load_transcript(&self, run_id: RunId) -> BoxFuture<'_, Result<RunTranscript, StoreError>> {
        let path = self.path();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let connection = open_connection(&path)?;
                let row = connection
                    .query_row(
                        "SELECT command_json, agent_json, status, terminal_json,
                                created_at_ms, finished_at_ms
                         FROM runs WHERE run_id = ?1",
                        [run_id.to_string()],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, Option<String>>(3)?,
                                row.get::<_, i64>(4)?,
                                row.get::<_, Option<i64>>(5)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(sqlite_error)?
                    .ok_or_else(|| StoreError::new(format!("run {run_id} was not found")))?;
                let command = serde_json::from_str(&row.0).map_err(json_error)?;
                let agent = serde_json::from_str(&row.1).map_err(json_error)?;
                let status = parse_status(&row.2)?;
                let terminal = row
                    .3
                    .map(|value| serde_json::from_str(&value).map_err(json_error))
                    .transpose()?;
                let run = RunRecord {
                    run_id,
                    command,
                    agent,
                    status,
                    terminal,
                    created_at_ms: row.4,
                    finished_at_ms: row.5,
                };

                let mut statement = connection
                    .prepare(
                        "SELECT event_id, sequence, recorded_at_ms, kind_json
                         FROM run_events WHERE run_id = ?1 ORDER BY sequence",
                    )
                    .map_err(sqlite_error)?;
                let event_rows = statement
                    .query_map([run_id.to_string()], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    })
                    .map_err(sqlite_error)?;
                let mut events = Vec::new();
                for event in event_rows {
                    let (event_id, sequence, recorded_at_ms, kind_json) =
                        event.map_err(sqlite_error)?;
                    events.push(RunEvent {
                        event_id: EventId::from_uuid(event_id.parse().map_err(|error| {
                            StoreError::new(format!("invalid event id: {error}"))
                        })?),
                        run_id,
                        sequence: u64::try_from(sequence)
                            .map_err(|_| StoreError::new("negative event sequence"))?,
                        recorded_at_ms,
                        kind: serde_json::from_str(&kind_json).map_err(json_error)?,
                    });
                }
                Ok(RunTranscript { run, events })
            })
            .await
            .map_err(join_error)?
        })
    }
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

fn load_open_cursor(
    transaction: &Transaction<'_>,
    run_id: RunId,
) -> Result<(RunStatus, u64), StoreError> {
    let row = transaction
        .query_row(
            "SELECT status, next_sequence FROM runs WHERE run_id = ?1",
            [run_id.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or_else(|| StoreError::new(format!("run {run_id} was not found")))?;
    let sequence = u64::try_from(row.1).map_err(|_| StoreError::new("negative event cursor"))?;
    Ok((parse_status(&row.0)?, sequence))
}

fn insert_event(
    transaction: &Transaction<'_>,
    run_id: RunId,
    sequence: u64,
    recorded_at_ms: i64,
    kind: &RunEventKind,
) -> Result<(), StoreError> {
    let event_id = EventId::new();
    let kind_json = serde_json::to_string(&kind).map_err(json_error)?;
    transaction
        .execute(
            "INSERT INTO run_events (
                event_id, run_id, sequence, recorded_at_ms, kind_json
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event_id.to_string(),
                run_id.to_string(),
                to_sql_integer(sequence)?,
                recorded_at_ms,
                kind_json,
            ],
        )
        .map_err(sqlite_error)?;
    Ok(())
}

fn now_ms() -> Result<i64, StoreError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| StoreError::new(format!("system clock is before Unix epoch: {error}")))?;
    i64::try_from(duration.as_millis()).map_err(|_| StoreError::new("timestamp exceeds i64"))
}

fn to_sql_integer(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::new("integer exceeds SQLite i64 range"))
}

const fn status_name(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Open => "open",
        RunStatus::Completed => "completed",
        RunStatus::Failed => "failed",
        RunStatus::Cancelled => "cancelled",
    }
}

fn parse_status(value: &str) -> Result<RunStatus, StoreError> {
    match value {
        "open" => Ok(RunStatus::Open),
        "completed" => Ok(RunStatus::Completed),
        "failed" => Ok(RunStatus::Failed),
        "cancelled" => Ok(RunStatus::Cancelled),
        value => Err(StoreError::new(format!("unknown run status: {value}"))),
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the owned signature is required by Result::map_err"
)]
fn sqlite_error(error: rusqlite::Error) -> StoreError {
    StoreError::new(format!("SQLite error: {error}"))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the owned signature is required by Result::map_err"
)]
fn json_error(error: serde_json::Error) -> StoreError {
    StoreError::new(format!("run serialization error: {error}"))
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the owned signature is required by Result::map_err"
)]
fn join_error(error: tokio::task::JoinError) -> StoreError {
    StoreError::new(format!("SQLite worker failed: {error}"))
}

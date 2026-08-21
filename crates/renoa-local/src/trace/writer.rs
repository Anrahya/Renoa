use std::{path::PathBuf, thread};

use renoa_kernel::{CommandId, SessionId};
use rusqlite::{Connection, params};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{TraceError, record::now_unix_ms, schema};

const TRACE_CHANNEL_CAPACITY: usize = 256;

pub(super) struct TraceWriter {
    pub(super) sender: mpsc::Sender<TraceCommand>,
    pub(super) join: thread::JoinHandle<Result<(), TraceError>>,
}

pub(super) struct TraceStart {
    pub(super) path: PathBuf,
    pub(super) session_id: SessionId,
    pub(super) run_id: Uuid,
    pub(super) command_id: CommandId,
    pub(super) started_at_ms: i64,
    pub(super) input_json: String,
    pub(super) provider: String,
    pub(super) model: String,
    pub(super) reasoning: String,
}

pub(super) enum TraceCommand {
    Entry(TraceEntry),
    Finish(TraceFinish),
}

pub(super) struct TraceFinish {
    pub(super) finished_at_ms: i64,
    pub(super) elapsed_us: i64,
    pub(super) status: String,
    pub(super) error_code: Option<String>,
    pub(super) error_message: Option<String>,
}

pub(super) struct TraceEntry {
    pub(super) sequence: i64,
    occurred_at_ms: i64,
    elapsed_us: i64,
    duration_us: Option<i64>,
    time_to_first_output_us: Option<i64>,
    component: String,
    kind: String,
    correlation_id: Option<String>,
    name: Option<String>,
    status: Option<String>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    payload_json: String,
}

impl TraceEntry {
    pub(super) fn new(
        component: impl Into<String>,
        kind: impl Into<String>,
        occurred_at_ms: i64,
        elapsed_us: i64,
    ) -> Self {
        Self {
            sequence: 0,
            occurred_at_ms,
            elapsed_us,
            duration_us: None,
            time_to_first_output_us: None,
            component: component.into(),
            kind: kind.into(),
            correlation_id: None,
            name: None,
            status: None,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_write_tokens: None,
            payload_json: "null".to_owned(),
        }
    }

    pub(super) fn correlation(mut self, value: impl Into<String>) -> Self {
        self.correlation_id = Some(value.into());
        self
    }

    pub(super) fn name(mut self, value: impl Into<String>) -> Self {
        self.name = Some(value.into());
        self
    }

    pub(super) fn status(mut self, value: Option<&str>) -> Self {
        self.status = value.map(str::to_owned);
        self
    }

    pub(super) fn duration(mut self, value: Option<i64>) -> Self {
        self.duration_us = value;
        self
    }

    pub(super) fn first_output(mut self, value: Option<i64>) -> Self {
        self.time_to_first_output_us = value;
        self
    }

    pub(super) fn usage(mut self, input: u64, output: u64, read: u64, write: u64) -> Self {
        self.input_tokens = i64::try_from(input).ok();
        self.output_tokens = i64::try_from(output).ok();
        self.cache_read_tokens = i64::try_from(read).ok();
        self.cache_write_tokens = i64::try_from(write).ok();
        self
    }

    pub(super) fn payload(mut self, value: &serde_json::Value) -> Self {
        self.payload_json = value.to_string();
        self
    }
}

impl TraceWriter {
    pub(super) fn start(start: TraceStart) -> Result<Self, TraceError> {
        let TraceStart {
            path,
            session_id,
            run_id,
            command_id,
            started_at_ms,
            input_json,
            provider,
            model,
            reasoning,
        } = start;
        let connection = schema::open(&path, session_id)?;
        schema::recover_running(&connection)?;
        connection.execute(
            "INSERT INTO runs(
                run_id, session_id, command_id, started_at_ms, status, trace_complete,
                provider, model, reasoning, input_json
             ) VALUES (?1, ?2, ?3, ?4, 'running', 0, ?5, ?6, ?7, ?8)",
            params![
                run_id.to_string(),
                session_id.to_string(),
                command_id.to_string(),
                started_at_ms,
                provider,
                model,
                reasoning,
                input_json
            ],
        )?;
        let (sender, receiver) = mpsc::channel(TRACE_CHANNEL_CAPACITY);
        let thread_name = format!("renoa-trace-{run_id}");
        let join = thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                WriterThread {
                    connection,
                    run_id,
                    receiver,
                }
                .run()
            })
            .map_err(|error| {
                if let Ok(connection) = schema::open(&path, session_id) {
                    let interrupted = TraceFinish {
                        finished_at_ms: now_unix_ms(),
                        elapsed_us: 0,
                        status: "interrupted".to_owned(),
                        error_code: Some("trace_writer_start_failed".to_owned()),
                        error_message: Some(error.to_string()),
                    };
                    if let Err(repair) = finish_run(&connection, run_id, &interrupted, false) {
                        return TraceError::WriterStartRepair {
                            source: error,
                            repair: repair.to_string(),
                        };
                    }
                }
                TraceError::WriterStart(error)
            })?;
        Ok(Self { sender, join })
    }
}

struct WriterThread {
    connection: Connection,
    run_id: Uuid,
    receiver: mpsc::Receiver<TraceCommand>,
}

impl WriterThread {
    fn run(self) -> Result<(), TraceError> {
        let Self {
            connection,
            run_id,
            mut receiver,
        } = self;
        while let Some(command) = receiver.blocking_recv() {
            match command {
                TraceCommand::Entry(entry) => insert_entry(&connection, run_id, &entry)?,
                TraceCommand::Finish(finish) => {
                    finish_run(&connection, run_id, &finish, true)?;
                    connection.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
                    return Ok(());
                }
            }
        }
        let interrupted = TraceFinish {
            finished_at_ms: now_unix_ms(),
            elapsed_us: 0,
            status: "interrupted".to_owned(),
            error_code: Some("trace_owner_dropped".to_owned()),
            error_message: Some("trace owner ended before finalizing the run".to_owned()),
        };
        finish_run(&connection, run_id, &interrupted, false)
    }
}

fn insert_entry(
    connection: &Connection,
    run_id: Uuid,
    entry: &TraceEntry,
) -> Result<(), TraceError> {
    connection.execute(
        "INSERT INTO events(
            run_id, sequence, occurred_at_ms, elapsed_us, duration_us,
            time_to_first_output_us, component, kind, correlation_id, name, status,
            input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, payload_json
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
         )",
        params![
            run_id.to_string(),
            entry.sequence,
            entry.occurred_at_ms,
            entry.elapsed_us,
            entry.duration_us,
            entry.time_to_first_output_us,
            entry.component,
            entry.kind,
            entry.correlation_id,
            entry.name,
            entry.status,
            entry.input_tokens,
            entry.output_tokens,
            entry.cache_read_tokens,
            entry.cache_write_tokens,
            entry.payload_json,
        ],
    )?;
    Ok(())
}

fn finish_run(
    connection: &Connection,
    run_id: Uuid,
    finish: &TraceFinish,
    complete: bool,
) -> Result<(), TraceError> {
    connection.execute(
        "UPDATE runs
         SET finished_at_ms = ?2, duration_us = ?3, status = ?4,
             trace_complete = ?5, error_code = ?6, error_message = ?7
         WHERE run_id = ?1 AND status = 'running'",
        params![
            run_id.to_string(),
            finish.finished_at_ms,
            finish.elapsed_us,
            finish.status,
            i64::from(complete),
            finish.error_code,
            finish.error_message,
        ],
    )?;
    Ok(())
}

//! Host-owned diagnostic trace for one local Renoa session.
//!
//! This store is deliberately separate from kernel recovery state. Trace data
//! can explain a run, but it never decides whether work may be replayed.

mod record;
mod schema;
mod writer;

#[cfg(test)]
mod tests;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};

use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock};
use renoa_kernel::{AgentId, CommandId, SessionId};
use thiserror::Error;
use tokio::sync::mpsc;
use uuid::Uuid;

use self::{
    record::{TraceState, now_unix_ms},
    writer::{TraceCommand, TraceEntry, TraceFinish, TraceStart, TraceWriter},
};
use crate::AgentProfileId;

pub(crate) const TRACE_DATABASE: &str = "trace.sqlite3";

#[derive(Debug, Error)]
pub(crate) enum TraceError {
    #[error("trace database failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("trace metadata is incompatible: {0}")]
    Incompatible(String),
    #[error("trace record could not be encoded: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("trace writer could not start: {0}")]
    WriterStart(#[source] std::io::Error),
    #[error("trace writer could not start: {source}; interrupted-run repair also failed: {repair}")]
    WriterStartRepair {
        #[source]
        source: std::io::Error,
        repair: String,
    },
    #[error("trace writer stopped before accepting all records")]
    WriterStopped,
    #[error("trace writer panicked")]
    WriterPanicked,
    #[error("trace join task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

#[derive(Clone)]
pub(crate) struct TraceStore {
    path: PathBuf,
    session_id: SessionId,
    agent_id: AgentId,
    profile_id: AgentProfileId,
}

impl TraceStore {
    pub(crate) fn create(
        path: PathBuf,
        session_id: SessionId,
        agent_id: AgentId,
        profile_id: &AgentProfileId,
    ) -> Result<Self, TraceError> {
        schema::create(&path, session_id, agent_id, profile_id)?;
        Ok(Self {
            path,
            session_id,
            agent_id,
            profile_id: profile_id.clone(),
        })
    }

    pub(crate) fn open(
        path: PathBuf,
        session_id: SessionId,
        agent_id: AgentId,
        profile_id: &AgentProfileId,
    ) -> Result<Self, TraceError> {
        let connection = schema::open(&path, session_id, agent_id, profile_id)?;
        schema::recover_running(&connection)?;
        Ok(Self {
            path,
            session_id,
            agent_id,
            profile_id: profile_id.clone(),
        })
    }

    pub(crate) async fn start_run(
        &self,
        command_id: CommandId,
        content: &[ContentBlock],
        provider: &str,
        model: &str,
        reasoning: &str,
    ) -> Result<Arc<TraceRun>, TraceError> {
        let path = self.path.clone();
        let session_id = self.session_id;
        let agent_id = self.agent_id;
        let profile_id = self.profile_id.clone();
        let run_id = Uuid::new_v4();
        let input = serde_json::to_string(content)?;
        let provider = provider.to_owned();
        let model = model.to_owned();
        let reasoning = reasoning.to_owned();
        let started_at_ms = now_unix_ms();
        let writer = tokio::task::spawn_blocking(move || {
            TraceWriter::start(TraceStart {
                path,
                session_id,
                agent_id,
                profile_id,
                run_id,
                command_id,
                started_at_ms,
                input_json: input,
                provider,
                model,
                reasoning,
            })
        })
        .await??;
        Ok(Arc::new(TraceRun::new(run_id, writer)))
    }
}

pub(crate) struct TraceRun {
    run_id: Uuid,
    started: Instant,
    sender: Mutex<Option<mpsc::Sender<TraceCommand>>>,
    join: Mutex<Option<std::thread::JoinHandle<Result<(), TraceError>>>>,
    state: tokio::sync::Mutex<TraceState>,
}

impl TraceRun {
    fn new(run_id: Uuid, writer: TraceWriter) -> Self {
        Self {
            run_id,
            started: Instant::now(),
            sender: Mutex::new(Some(writer.sender)),
            join: Mutex::new(Some(writer.join)),
            state: tokio::sync::Mutex::new(TraceState::default()),
        }
    }

    pub(crate) async fn record_host(
        &self,
        kind: &str,
        status: Option<&str>,
        payload: serde_json::Value,
    ) -> Result<(), TraceError> {
        self.record_entry(
            TraceEntry::new("host", kind, now_unix_ms(), self.elapsed_us())
                .correlation(self.run_id.to_string())
                .status(status)
                .payload(&payload),
        )
        .await
    }

    pub(crate) async fn finish(
        &self,
        status: &str,
        error_code: Option<&str>,
        error_message: Option<&str>,
    ) -> Result<(), TraceError> {
        let mut state = self.state.lock().await;
        let sender = self.sender()?;
        sender
            .send(TraceCommand::Finish(TraceFinish {
                finished_at_ms: now_unix_ms(),
                elapsed_us: self.elapsed_us(),
                status: status.to_owned(),
                error_code: error_code.map(str::to_owned),
                error_message: error_message.map(str::to_owned),
            }))
            .await
            .map_err(|_| TraceError::WriterStopped)?;
        lock_unpoisoned(&self.sender).take();
        state.finished = true;
        drop(state);
        let join = lock_unpoisoned(&self.join).take();
        if let Some(join) = join {
            tokio::task::spawn_blocking(move || join.join())
                .await?
                .map_err(|_| TraceError::WriterPanicked)??;
        }
        Ok(())
    }

    async fn observe(&self, event: AgentEvent) -> Result<(), TraceError> {
        let elapsed_us = self.elapsed_us();
        let mut state = self.state.lock().await;
        let entry = state.agent_event(event, now_unix_ms(), elapsed_us);
        self.send_locked(&mut state, entry).await
    }

    async fn record_entry(&self, entry: TraceEntry) -> Result<(), TraceError> {
        let mut state = self.state.lock().await;
        self.send_locked(&mut state, entry).await
    }

    async fn send_locked(
        &self,
        state: &mut TraceState,
        mut entry: TraceEntry,
    ) -> Result<(), TraceError> {
        if state.finished {
            return Err(TraceError::WriterStopped);
        }
        entry.sequence = state.next_sequence()?;
        self.sender()?
            .send(TraceCommand::Entry(entry))
            .await
            .map_err(|_| TraceError::WriterStopped)
    }

    fn sender(&self) -> Result<mpsc::Sender<TraceCommand>, TraceError> {
        lock_unpoisoned(&self.sender)
            .clone()
            .ok_or(TraceError::WriterStopped)
    }

    fn elapsed_us(&self) -> i64 {
        i64::try_from(self.started.elapsed().as_micros()).unwrap_or(i64::MAX)
    }
}

impl AgentEventSink for TraceRun {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if let Err(error) = self.observe(event).await {
                eprintln!("Renoa trace event could not be recorded: {error}");
            }
        })
    }
}

impl Drop for TraceRun {
    fn drop(&mut self) {
        lock_unpoisoned(&self.sender).take();
        if let Some(join) = lock_unpoisoned(&self.join).take()
            && join.join().is_err()
        {
            eprintln!("Renoa trace writer panicked while closing an unfinished run");
        }
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) struct ObservedEventSink {
    trace: Arc<TraceRun>,
    surface: Arc<dyn AgentEventSink>,
}

impl ObservedEventSink {
    pub(crate) fn new(trace: Arc<TraceRun>, surface: Arc<dyn AgentEventSink>) -> Self {
        Self { trace, surface }
    }
}

impl AgentEventSink for ObservedEventSink {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            self.trace.emit(event.clone()).await;
            self.surface.emit(event).await;
        })
    }
}

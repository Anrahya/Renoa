use serde::{Deserialize, Serialize};

use crate::{
    BoxFuture, CapabilityCall, CapabilityOutcome, CommandEnvelope, EventId, ModelResponse,
    ResolvedAgent, RunId, StoreError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Open,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TerminalState {
    Completed { output: String },
    Failed { error: String },
    Cancelled { reason: String },
}

impl TerminalState {
    #[must_use]
    pub const fn status(&self) -> RunStatus {
        match self {
            Self::Completed { .. } => RunStatus::Completed,
            Self::Failed { .. } => RunStatus::Failed,
            Self::Cancelled { .. } => RunStatus::Cancelled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEventKind {
    RunStarted {
        command: CommandEnvelope,
        agent: ResolvedAgent,
    },
    ModelRequested {
        round: u32,
    },
    ModelResponded {
        round: u32,
        response: ModelResponse,
    },
    CapabilityRequested {
        ordinal: u32,
        call: CapabilityCall,
    },
    CapabilityCompleted {
        ordinal: u32,
        call_id: String,
        outcome: CapabilityOutcome,
    },
    RunTerminated {
        terminal: TerminalState,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunEvent {
    pub event_id: EventId,
    pub run_id: RunId,
    pub sequence: u64,
    pub recorded_at_ms: i64,
    pub kind: RunEventKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub run_id: RunId,
    pub command: CommandEnvelope,
    pub agent: ResolvedAgent,
    pub status: RunStatus,
    pub terminal: Option<TerminalState>,
    pub created_at_ms: i64,
    pub finished_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunTranscript {
    pub run: RunRecord,
    pub events: Vec<RunEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunAdmission {
    Admitted(RunId),
    Existing(RunId),
    Conflict(RunId),
}

/// Persists the canonical run ledger and its ordered event stream.
///
/// `admit_run` must atomically admit each command identity once. Only the
/// caller receiving `RunAdmission::Admitted` owns execution of that run.
///
/// `finish_run` must implement an open-to-terminal compare-and-set so only one
/// terminal writer can win.
pub trait RunStore: Send + Sync {
    fn admit_run(
        &self,
        command: CommandEnvelope,
        agent: ResolvedAgent,
    ) -> BoxFuture<'_, Result<RunAdmission, StoreError>>;

    fn append_events(
        &self,
        run_id: RunId,
        events: Vec<RunEventKind>,
    ) -> BoxFuture<'_, Result<(), StoreError>>;

    fn finish_run(
        &self,
        run_id: RunId,
        terminal: TerminalState,
    ) -> BoxFuture<'_, Result<(), StoreError>>;

    fn load_transcript(&self, run_id: RunId) -> BoxFuture<'_, Result<RunTranscript, StoreError>>;
}

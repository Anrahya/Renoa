use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ExecutionEventId, ExecutionId};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionEvent {
    pub event_id: ExecutionEventId,
    pub execution_id: ExecutionId,
    pub sequence: u64,
    pub recorded_at_ms: i64,
    pub kind: ExecutionEventKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionEventKind {
    ExecutionStarted,
    TurnStarted,
    AssistantMessage {
        text: String,
    },
    ToolStarted {
        call_id: String,
        name: String,
        arguments: Value,
    },
    ToolFinished {
        call_id: String,
        output: String,
        is_error: bool,
    },
    ExecutionTerminated {
        terminal: ExecutionTerminal,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ExecutionTerminal {
    Completed,
    Failed { error: String },
    Cancelled { reason: String },
}

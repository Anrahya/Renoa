use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{BoxFuture, ContentBlock};

/// How calls in one assistant tool batch may be scheduled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolExecutionMode {
    #[default]
    Sequential,
    Parallel,
}

/// Provider-neutral definition advertised to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// One model-requested tool invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

/// Model-visible outcome of one tool invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub name: String,
    pub content: Vec<ContentBlock>,
    pub details: Option<Value>,
    pub is_error: bool,
}

/// Final or partial output produced by a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub content: Vec<ContentBlock>,
    pub details: Option<Value>,
}

/// Bounded progress channel scoped to one tool execution.
#[derive(Clone)]
pub struct ToolUpdates {
    sender: mpsc::Sender<ToolOutput>,
    accepting: Arc<AtomicBool>,
}

impl ToolUpdates {
    pub(crate) fn channel() -> (Self, mpsc::Receiver<ToolOutput>) {
        let (sender, receiver) = mpsc::channel(1);
        (
            Self {
                sender,
                accepting: Arc::new(AtomicBool::new(true)),
            },
            receiver,
        )
    }

    /// Emits progress with backpressure. Updates after execution settles are ignored.
    pub async fn emit(&self, update: ToolOutput) {
        if self.accepting.load(Ordering::Acquire) {
            let _ = self.sender.send(update).await;
        }
    }

    pub(crate) fn close(&self) {
        self.accepting.store(false, Ordering::Release);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct ToolError {
    message: String,
}

impl ToolError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Host-provided behavior callable by the agent loop.
pub trait Tool: Send + Sync {
    fn spec(&self) -> &ToolSpec;

    /// Declares whether this tool can share a tool batch with concurrent work.
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Parallel
    }

    /// Returns ordered model-visible content or a model-visible error.
    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>>;
}

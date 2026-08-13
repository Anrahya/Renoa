use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock, events::emit_event};

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
    ///
    /// The future must observe `cancellation` and resolve only after work it
    /// started is stopped. Process tools, for example, must kill and reap their
    /// process group before returning. A tool that detaches work violates this
    /// contract.
    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>>;
}

/// Executes one tool call, drains bounded progress, and returns one
/// model-visible result. On cancellation, this waits for the tool future to
/// confirm its work has stopped. Callers own start/end lifecycle events.
pub async fn invoke_tool(
    tool: Option<&dyn Tool>,
    call: ToolCall,
    cancellation: CancellationToken,
    sink: Option<&dyn AgentEventSink>,
) -> ToolResult {
    if cancellation.is_cancelled() {
        return error_tool_result(&call, "Tool execution was cancelled.");
    }
    let Some(tool) = tool else {
        return error_tool_result(&call, &format!("Tool `{}` is not available.", call.name));
    };

    let (updates, mut receiver) = ToolUpdates::channel();
    let mut execution = tool.execute(call.clone(), cancellation.child_token(), updates.clone());
    let mut cancelled = false;
    let outcome = loop {
        tokio::select! {
            biased;
            () = cancellation.cancelled(), if !cancelled => {
                cancelled = true;
                updates.close();
            },
            Some(update) = receiver.recv() => emit_event(
                sink,
                AgentEvent::ToolExecutionUpdate { call: call.clone(), update },
            ).await,
            outcome = &mut execution => break outcome,
        }
    };
    updates.close();
    receiver.close();
    while let Ok(update) = receiver.try_recv() {
        emit_event(
            sink,
            AgentEvent::ToolExecutionUpdate {
                call: call.clone(),
                update,
            },
        )
        .await;
    }

    match (cancelled, outcome) {
        (true, _) => error_tool_result(&call, "Tool execution was cancelled."),
        (false, Ok(output)) => ToolResult {
            call_id: call.id,
            name: call.name,
            content: output.content,
            details: output.details,
            is_error: false,
        },
        (false, Err(error)) => error_tool_result(&call, &error.to_string()),
    }
}

pub(crate) fn error_tool_result(call: &ToolCall, message: &str) -> ToolResult {
    ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: vec![ContentBlock::text(message)],
        details: None,
        is_error: true,
    }
}

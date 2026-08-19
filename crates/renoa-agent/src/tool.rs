use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
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

/// An invalid set of tool calls from one completed model response.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ToolCallBatchError {
    #[error("model returned a tool call with an empty identifier")]
    EmptyId,
    #[error("model returned duplicate tool-call identifier `{0}`")]
    DuplicateId(String),
}

/// Validates identifiers that must be unambiguous within one tool-call batch.
///
/// # Errors
///
/// Returns [`ToolCallBatchError::EmptyId`] for an empty identifier or
/// [`ToolCallBatchError::DuplicateId`] when two calls have the same identifier.
pub fn validate_tool_call_ids<'a>(
    identifiers: impl IntoIterator<Item = &'a str>,
) -> Result<(), ToolCallBatchError> {
    let identifiers = identifiers.into_iter();
    let mut observed = HashSet::with_capacity(identifiers.size_hint().0);
    for identifier in identifiers {
        if identifier.is_empty() {
            return Err(ToolCallBatchError::EmptyId);
        }
        if !observed.insert(identifier) {
            return Err(ToolCallBatchError::DuplicateId(identifier.to_owned()));
        }
    }
    Ok(())
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
    certainty: ToolErrorCertainty,
}

impl ToolError {
    /// Creates a definite failure that may be returned to the model.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            certainty: ToolErrorCertainty::Definite,
        }
    }

    /// Creates an error for an invocation whose external outcome cannot be proven.
    #[must_use]
    pub fn outcome_unknown(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            certainty: ToolErrorCertainty::OutcomeUnknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolErrorCertainty {
    Definite,
    OutcomeUnknown,
}

/// Evidence that a tool invocation may have completed without a known result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
#[error("tool `{tool_name}` call `{call_id}` has an unknown outcome: {message}")]
pub struct ToolOutcomeUnknown {
    call_id: String,
    tool_name: String,
    message: String,
}

impl ToolOutcomeUnknown {
    #[must_use]
    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    #[must_use]
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Host-provided behavior callable by the agent loop.
pub trait Tool: Send + Sync {
    fn spec(&self) -> &ToolSpec;

    /// Declares whether this tool can share a tool batch with concurrent work.
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Parallel
    }

    /// Returns ordered model-visible content, a definite model-visible error,
    /// or [`ToolError::outcome_unknown`] when the final external outcome cannot
    /// be proven.
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

/// Executes one tool call and drains bounded progress.
///
/// Definite tool failures become model-visible error results. Uncertain
/// failures remain typed errors so a durable caller can stop without recording
/// a false result. On cancellation, this waits for the tool future to confirm
/// its work has stopped. Callers own lifecycle events; an uncertain outcome has
/// no settled end event.
///
/// # Errors
///
/// Returns [`ToolOutcomeUnknown`] when the tool cannot prove its final external
/// outcome.
pub async fn invoke_tool(
    tool: Option<&dyn Tool>,
    call: ToolCall,
    cancellation: CancellationToken,
    sink: Option<&dyn AgentEventSink>,
) -> Result<ToolResult, ToolOutcomeUnknown> {
    if cancellation.is_cancelled() {
        return Ok(error_tool_result(&call, "Tool execution was cancelled."));
    }
    let Some(tool) = tool else {
        return Ok(error_tool_result(
            &call,
            &format!("Tool `{}` is not available.", call.name),
        ));
    };

    let (updates, mut receiver) = ToolUpdates::channel();
    let mut execution = tool.execute(call.clone(), cancellation.child_token(), updates.clone());
    let mut cancellation_observed = false;
    let outcome = loop {
        tokio::select! {
            biased;
            () = cancellation.cancelled(), if !cancellation_observed => {
                cancellation_observed = true;
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

    match outcome {
        Ok(output) => Ok(ToolResult {
            call_id: call.id,
            name: call.name,
            content: output.content,
            details: output.details,
            is_error: false,
        }),
        Err(error) => match error.certainty {
            ToolErrorCertainty::Definite => Ok(error_tool_result(&call, &error.message)),
            ToolErrorCertainty::OutcomeUnknown => Err(ToolOutcomeUnknown {
                call_id: call.id,
                tool_name: call.name,
                message: error.message,
            }),
        },
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

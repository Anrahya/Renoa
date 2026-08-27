use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock, events::emit_event};

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
    /// Durable structured output for Host inspection, excluded from model requests.
    pub details: Option<Value>,
    pub is_error: bool,
}

/// Final or partial output produced by a tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolOutput {
    pub content: Vec<ContentBlock>,
    pub details: Option<Value>,
    /// Whether this completed output is a model-visible tool error.
    ///
    /// This is distinct from [`ToolError`]: a remote tool may complete with
    /// useful ordered error content that must be preserved as its result.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
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
    code: ToolErrorCode,
    message: String,
    certainty: ToolErrorCertainty,
    partial_changes_possible: bool,
}

impl ToolError {
    /// Creates an unclassified definite failure that may be returned to the model.
    ///
    /// New adapters should use a category-specific constructor whenever the
    /// failure is understood. This constructor remains for implementations
    /// whose external error vocabulary has not yet been mapped.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self::definite(ToolErrorCode::Internal, message, false)
    }

    #[must_use]
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::definite(ToolErrorCode::InvalidInput, message, false)
    }

    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::definite(ToolErrorCode::NotFound, message, false)
    }

    #[must_use]
    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::definite(ToolErrorCode::PermissionDenied, message, false)
    }

    #[must_use]
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::definite(ToolErrorCode::Conflict, message, false)
    }

    #[must_use]
    pub fn timeout(message: impl Into<String>, partial_changes_possible: bool) -> Self {
        Self::definite(ToolErrorCode::Timeout, message, partial_changes_possible)
    }

    #[must_use]
    pub fn cancelled(message: impl Into<String>, partial_changes_possible: bool) -> Self {
        Self::definite(ToolErrorCode::Cancelled, message, partial_changes_possible)
    }

    #[must_use]
    pub fn process_failed(message: impl Into<String>, partial_changes_possible: bool) -> Self {
        Self::definite(
            ToolErrorCode::ProcessFailed,
            message,
            partial_changes_possible,
        )
    }

    #[must_use]
    pub fn output_limit(message: impl Into<String>) -> Self {
        Self::definite(ToolErrorCode::OutputLimit, message, false)
    }

    #[must_use]
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::definite(ToolErrorCode::Unavailable, message, false)
    }

    #[must_use]
    pub fn io(message: impl Into<String>, partial_changes_possible: bool) -> Self {
        Self::definite(ToolErrorCode::Io, message, partial_changes_possible)
    }

    #[must_use]
    pub fn internal(message: impl Into<String>) -> Self {
        Self::definite(ToolErrorCode::Internal, message, false)
    }

    /// Creates an error for an invocation whose external outcome cannot be proven.
    #[must_use]
    pub fn outcome_unknown(message: impl Into<String>) -> Self {
        Self {
            code: ToolErrorCode::Unavailable,
            message: message.into(),
            certainty: ToolErrorCertainty::OutcomeUnknown,
            partial_changes_possible: true,
        }
    }

    #[must_use]
    pub fn code(&self) -> ToolErrorCode {
        self.code
    }

    #[must_use]
    pub fn outcome_is_unknown(&self) -> bool {
        self.certainty == ToolErrorCertainty::OutcomeUnknown
    }

    #[must_use]
    pub fn partial_changes_possible(&self) -> bool {
        self.partial_changes_possible
    }

    fn definite(
        code: ToolErrorCode,
        message: impl Into<String>,
        partial_changes_possible: bool,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            certainty: ToolErrorCertainty::Definite,
            partial_changes_possible,
        }
    }
}

/// Stable model-visible category of a definite tool failure.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolErrorCode {
    InvalidInput,
    NotFound,
    PermissionDenied,
    Conflict,
    Timeout,
    Cancelled,
    ProcessFailed,
    OutputLimit,
    Unavailable,
    Io,
    #[default]
    Internal,
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
    #[serde(default)]
    code: ToolErrorCode,
    message: String,
    #[serde(default)]
    partial_changes_possible: bool,
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

    #[must_use]
    pub fn code(&self) -> ToolErrorCode {
        self.code
    }

    #[must_use]
    pub fn partial_changes_possible(&self) -> bool {
        self.partial_changes_possible
    }
}

/// Host-provided behavior callable by the agent loop.
pub trait Tool: Send + Sync {
    fn spec(&self) -> &ToolSpec;

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
        return Ok(error_tool_result(
            &call,
            &ToolError::cancelled("Tool execution was cancelled.", false),
        ));
    }
    let Some(tool) = tool else {
        return Ok(error_tool_result(
            &call,
            &ToolError::unavailable(format!("Tool `{}` is not available.", call.name)),
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
            is_error: output.is_error,
        }),
        Err(error) => match error.certainty {
            ToolErrorCertainty::Definite => Ok(error_tool_result(&call, &error)),
            ToolErrorCertainty::OutcomeUnknown => Err(ToolOutcomeUnknown {
                call_id: call.id,
                tool_name: call.name,
                code: error.code,
                message: error.message,
                partial_changes_possible: error.partial_changes_possible,
            }),
        },
    }
}

pub(crate) fn error_tool_result(call: &ToolCall, error: &ToolError) -> ToolResult {
    ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: vec![ContentBlock::text(error.to_string())],
        details: Some(json!({
            "error": {
                "code": error.code,
                "partial_changes_possible": error.partial_changes_possible
            }
        })),
        is_error: true,
    }
}

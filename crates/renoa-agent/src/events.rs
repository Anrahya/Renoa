use std::collections::BTreeMap;

use serde::Serialize;

use crate::{
    AssistantDelta, BoxFuture, Message, MessageRole, ModelErrorKind, ModelFailureDiagnostic,
    ModelRequest, ModelResponse, ToolCall, ToolOutcomeUnknown, ToolOutput, ToolResult,
};

/// Stable diagnostic category for one unsuccessful model invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ModelFailureCode {
    Authentication,
    RateLimited,
    InvalidRequest,
    ContextWindowExceeded,
    Network,
    Timeout,
    ProviderUnavailable,
    Protocol,
    StreamInterrupted,
    Cancelled,
    IncompleteStream,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    AgentStart,
    TurnStart,
    MessageStart {
        role: MessageRole,
    },
    MessageUpdate {
        content_index: usize,
        delta: AssistantDelta,
    },
    MessageAbort,
    MessageEnd {
        message: Message,
    },
    ModelRequestStart {
        invocation_id: String,
        request: ModelRequest,
    },
    ModelProviderRequest {
        invocation_id: String,
        payload: serde_json::Value,
    },
    ModelProviderResponse {
        invocation_id: String,
        status: u16,
        headers: BTreeMap<String, String>,
    },
    ModelRequestChunk {
        invocation_id: String,
        content_index: usize,
        delta: AssistantDelta,
    },
    ModelRequestEnd {
        invocation_id: String,
        response: ModelResponse,
    },
    ModelRequestFailed {
        invocation_id: String,
        code: ModelFailureCode,
        message: String,
        outcome_unknown: bool,
        diagnostic: Option<ModelFailureDiagnostic>,
    },
    ModelRetryAttempt {
        invocation_id: String,
        attempt: u32,
        next_attempt: u32,
        category: ModelErrorKind,
        delay_ms: u64,
        cause_code: Option<String>,
    },
    ToolExecutionStart {
        call: ToolCall,
    },
    ToolExecutionUpdate {
        call: ToolCall,
        update: ToolOutput,
    },
    ToolExecutionEnd {
        call: ToolCall,
        result: ToolResult,
    },
    ToolExecutionOutcomeUnknown {
        call: ToolCall,
        error: ToolOutcomeUnknown,
    },
    TurnEnd,
    AgentEnd,
}

/// Host-owned observer for transient model and tool progress.
///
/// Emission is awaited inline to preserve order and backpressure. Implementors
/// must return promptly and must not treat these events as authoritative
/// history; a dropped execution may end delivery without a terminal event.
pub trait AgentEventSink: Send + Sync {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()>;
}

pub(crate) async fn emit_event(sink: Option<&dyn AgentEventSink>, event: AgentEvent) {
    if let Some(sink) = sink {
        sink.emit(event).await;
    }
}

pub(crate) async fn append_message(
    sink: Option<&dyn AgentEventSink>,
    messages: &mut Vec<Message>,
    message: Message,
) {
    finish_message(sink, messages, message, false).await;
}

pub(crate) async fn finish_message(
    sink: Option<&dyn AgentEventSink>,
    messages: &mut Vec<Message>,
    message: Message,
    already_started: bool,
) {
    if !already_started {
        emit_event(
            sink,
            AgentEvent::MessageStart {
                role: message.role(),
            },
        )
        .await;
    }
    messages.push(message.clone());
    emit_event(sink, AgentEvent::MessageEnd { message }).await;
}

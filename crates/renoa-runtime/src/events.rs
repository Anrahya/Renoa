use renoa_core::{BoxFuture, CapabilityCall, CapabilityOutcome, Message};

#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    AgentStart,
    TurnStart,
    MessageStart {
        message: Message,
    },
    MessageUpdate {
        text_delta: String,
    },
    MessageAbort,
    MessageEnd {
        message: Message,
    },
    ToolExecutionStart {
        call: CapabilityCall,
    },
    ToolExecutionEnd {
        call: CapabilityCall,
        outcome: CapabilityOutcome,
    },
    TurnEnd,
    AgentEnd,
}

/// Receives ordered, transient lifecycle events from an [`Agent`](crate::Agent).
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
                message: message.clone(),
            },
        )
        .await;
    }
    messages.push(message.clone());
    emit_event(sink, AgentEvent::MessageEnd { message }).await;
}

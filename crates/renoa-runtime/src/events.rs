use renoa_core::{BoxFuture, CapabilityCall, CapabilityOutcome, Message};

#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    AgentStart,
    TurnStart,
    MessageStart {
        message: Message,
    },
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
    emit_event(
        sink,
        AgentEvent::MessageStart {
            message: message.clone(),
        },
    )
    .await;
    messages.push(message.clone());
    emit_event(sink, AgentEvent::MessageEnd { message }).await;
}

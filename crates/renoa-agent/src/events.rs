use crate::{AssistantDelta, BoxFuture, Message, MessageRole, ToolCall, ToolOutput, ToolResult};

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
    TurnEnd,
    AgentEnd,
}

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

use renoa_agent::{
    AgentEvent, AgentEventSink, AssistantContent, BoxFuture, ContentBlock, Message, ToolResult,
};
use renoa_local::LocalTurnOutcome;
use renoa_protocol::{ExecutionEventKind, ExecutionTerminal};

use crate::NodeError;

pub(crate) fn project_history(
    messages: impl IntoIterator<Item = Message>,
) -> Result<Vec<ExecutionEventKind>, NodeError> {
    let mut events = Vec::new();
    for message in messages {
        match message {
            Message::User { .. } => {}
            Message::Assistant { content, .. } => {
                for block in content {
                    match block {
                        AssistantContent::Text { text, .. } => {
                            events.push(ExecutionEventKind::AssistantMessage { text });
                        }
                        AssistantContent::Reasoning { .. } => {}
                        AssistantContent::ToolCall { call } => {
                            events.push(ExecutionEventKind::ToolStarted {
                                call_id: call.id,
                                name: call.name,
                                arguments: call.arguments,
                            });
                        }
                    }
                }
            }
            Message::Tool { result } => events.push(project_tool_result(result)?),
        }
    }
    Ok(events)
}

fn project_tool_result(result: ToolResult) -> Result<ExecutionEventKind, NodeError> {
    let output = match result.content.as_slice() {
        [ContentBlock::Text { text }] => text.clone(),
        content => serde_json::to_string(content)?,
    };
    Ok(ExecutionEventKind::ToolFinished {
        call_id: result.call_id,
        output,
        is_error: result.is_error,
    })
}

pub(crate) fn terminal_event(outcome: LocalTurnOutcome) -> ExecutionEventKind {
    let terminal = match outcome {
        LocalTurnOutcome::Completed { .. }
        | LocalTurnOutcome::Compacted { .. }
        | LocalTurnOutcome::WaitingForInput => ExecutionTerminal::Completed,
        LocalTurnOutcome::Cancelled => ExecutionTerminal::Cancelled {
            reason: "agent turn was cancelled".to_owned(),
        },
        LocalTurnOutcome::Failed { reason } => ExecutionTerminal::Failed { error: reason },
        _ => ExecutionTerminal::Failed {
            error: "local Host returned an unsupported turn outcome".to_owned(),
        },
    };
    ExecutionEventKind::ExecutionTerminated { terminal }
}

pub(crate) struct NoopEvents;

impl AgentEventSink for NoopEvents {
    fn emit(&self, _event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }
}

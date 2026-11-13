use std::collections::HashSet;

use renoa_agent::{AssistantContent, ContentBlock, Message, ToolCall, ToolResult};
use renoa_local::LocalTurnOutcome;
use renoa_protocol::{ExecutionEventKind, ExecutionTerminal};

use crate::NodeError;

pub(crate) fn project_history(
    messages: impl IntoIterator<Item = Message>,
    synthetic_activity_tools: &HashSet<String>,
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
                            let synthetic = synthetic_activity_tools.contains(&call.name);
                            events.push(project_tool_start(&call, synthetic));
                        }
                    }
                }
            }
            Message::Tool { result } => events.push(project_tool_result(result)?),
        }
    }
    Ok(events)
}

pub(crate) fn project_tool_start(call: &ToolCall, synthetic_activity: bool) -> ExecutionEventKind {
    if synthetic_activity {
        ExecutionEventKind::ToolStarted {
            call_id: call.id.clone(),
            name: call.name.clone(),
            activity: renoa_agent::extract_tool_activity(&call.arguments),
            arguments: renoa_agent::strip_activity_field(&call.arguments),
        }
    } else {
        ExecutionEventKind::ToolStarted {
            call_id: call.id.clone(),
            name: call.name.clone(),
            activity: None,
            arguments: call.arguments.clone(),
        }
    }
}

pub(crate) fn project_tool_result(result: ToolResult) -> Result<ExecutionEventKind, NodeError> {
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

/// What a surface shows after the owner's `/compact` wrote a summary: the
/// agent says nothing for a summary, so the node reports the one fact the owner
/// asked for.
pub(crate) const COMPACTED_NOTICE: &str =
    "Earlier messages are summarized now. This conversation continues from that summary.";

/// What a surface shows after a `/compact` that found nothing new to summarize.
pub(crate) const UP_TO_DATE_NOTICE: &str = "There was nothing new to summarize.";

/// The records that close a command: the compaction notice when the command
/// was a compaction, then its terminal record.
pub(crate) fn closing_events(outcome: LocalTurnOutcome) -> Vec<ExecutionEventKind> {
    let mut events = Vec::new();
    if let LocalTurnOutcome::Compacted { summarized, .. } = outcome {
        events.push(ExecutionEventKind::AssistantMessage {
            text: if summarized {
                COMPACTED_NOTICE
            } else {
                UP_TO_DATE_NOTICE
            }
            .to_owned(),
        });
    }
    events.push(terminal_event(outcome));
    events
}

fn terminal_event(outcome: LocalTurnOutcome) -> ExecutionEventKind {
    let terminal = match outcome {
        LocalTurnOutcome::Completed { .. }
        | LocalTurnOutcome::Compacted { .. }
        | LocalTurnOutcome::WaitingForInput { .. } => ExecutionTerminal::Completed,
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use renoa_agent::{AssistantContent, AssistantMetadata, Message, StopReason, ToolCall};
    use renoa_protocol::ExecutionEventKind;
    use serde_json::json;

    use super::project_history;

    #[test]
    fn project_history_only_extracts_and_strips_synthetic_activity() {
        let messages = [Message::Assistant {
            content: vec![
                AssistantContent::tool_call(ToolCall {
                    id: "c1".to_owned(),
                    name: "read_file".to_owned(),
                    arguments: json!({ "path": "Cargo.toml", "activity": "Reading manifest" }),
                    thought_signature: None,
                    namespace: None,
                }),
                AssistantContent::tool_call(ToolCall {
                    id: "c2".to_owned(),
                    name: "log_workout".to_owned(),
                    arguments: json!({ "activity": "Morning run" }),
                    thought_signature: None,
                    namespace: None,
                }),
            ],
            stop_reason: StopReason::ToolUse,
            usage: None,
            metadata: AssistantMetadata::default(),
        }];

        let synthetic = HashSet::from(["read_file".to_owned()]);
        let events = project_history(messages, &synthetic).expect("project history");
        assert_eq!(
            events,
            vec![
                ExecutionEventKind::ToolStarted {
                    call_id: "c1".to_owned(),
                    name: "read_file".to_owned(),
                    activity: Some("Reading manifest".to_owned()),
                    arguments: json!({ "path": "Cargo.toml" }),
                },
                ExecutionEventKind::ToolStarted {
                    call_id: "c2".to_owned(),
                    name: "log_workout".to_owned(),
                    activity: None,
                    arguments: json!({ "activity": "Morning run" }),
                },
            ]
        );
    }
}

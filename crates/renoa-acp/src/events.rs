use std::sync::Mutex;

use crate::ServerError;
use agent_client_protocol::{
    Client, ConnectionTo,
    schema::v1::{
        ContentBlock as AcpContentBlock, ContentChunk, ImageContent, MessageId, Meta, SessionId,
        SessionNotification, SessionUpdate, TextContent, ToolCall as AcpToolCall, ToolCallContent,
        ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
    },
};
use renoa_agent::{
    AgentEvent, AgentEventSink, AssistantContent, AssistantDelta, BoxFuture, ContentBlock, Message,
    MessageRole, ToolCall, ToolOutput, ToolResult,
};
use renoa_local::LocalHistoryEntry;
use uuid::Uuid;

pub(crate) struct AcpEventSink {
    connection: ConnectionTo<Client>,
    session_id: SessionId,
    state: Mutex<EventState>,
}

struct EventState {
    current_text: String,
    message_id: String,
    send_error: Option<String>,
}

impl AcpEventSink {
    pub(crate) fn new(connection: ConnectionTo<Client>, session_id: impl Into<SessionId>) -> Self {
        Self {
            connection,
            session_id: session_id.into(),
            state: Mutex::new(EventState {
                current_text: String::new(),
                message_id: Uuid::new_v4().to_string(),
                send_error: None,
            }),
        }
    }

    pub(crate) fn remaining_chunk(
        &self,
        output: &str,
    ) -> Result<Option<ContentChunk>, ServerError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ServerError::Operation("ACP event state was poisoned".to_owned()))?;
        if let Some(error) = &state.send_error {
            return Err(ServerError::Operation(format!(
                "ACP event delivery failed: {error}"
            )));
        }
        if state.current_text == output {
            return Ok(None);
        }
        let text = output.strip_prefix(&state.current_text).ok_or_else(|| {
            ServerError::Operation(
                "streamed assistant text differs from the durable response".to_owned(),
            )
        })?;
        if text.is_empty() {
            return Ok(None);
        }
        state.current_text.push_str(text);
        Ok(Some(text_chunk(text.to_owned(), &state.message_id)))
    }

    pub(crate) fn ensure_delivery(&self) -> Result<(), ServerError> {
        let state = self
            .state
            .lock()
            .map_err(|_| ServerError::Operation("ACP event state was poisoned".to_owned()))?;
        state.send_error.as_ref().map_or(Ok(()), |error| {
            Err(ServerError::Operation(format!(
                "ACP event delivery failed: {error}"
            )))
        })
    }

    fn send(&self, update: SessionUpdate) {
        if let Err(error) = self
            .connection
            .send_notification(SessionNotification::new(self.session_id.clone(), update))
            && let Ok(mut state) = self.state.lock()
            && state.send_error.is_none()
        {
            state.send_error = Some(error.to_string());
        }
    }

    fn observe(&self, event: AgentEvent) {
        match event {
            AgentEvent::ModelRequestStart { .. }
            | AgentEvent::MessageStart {
                role: MessageRole::Assistant,
            } => self.start_assistant_message(),
            AgentEvent::MessageAbort => self.clear_text(),
            AgentEvent::MessageUpdate { delta, .. } => self.send_delta(delta),
            AgentEvent::ToolExecutionStart { call } => self.send(SessionUpdate::ToolCall(
                AcpToolCall::new(call.id.clone(), tool_title(&call))
                    .kind(tool_kind(&call.name))
                    .status(ToolCallStatus::InProgress)
                    .raw_input(call.arguments),
            )),
            AgentEvent::ToolExecutionUpdate { call, update } => {
                self.send(SessionUpdate::ToolCallUpdate(tool_update(call.id, update)));
            }
            AgentEvent::ToolExecutionEnd { call, result } => {
                self.send(SessionUpdate::ToolCallUpdate(tool_result(call, result)));
            }
            AgentEvent::ToolExecutionOutcomeUnknown { call, error } => {
                self.send(SessionUpdate::ToolCallUpdate(tool_outcome_unknown(
                    call.id, &error,
                )));
            }
            _ => {}
        }
    }

    fn clear_text(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.current_text.clear();
        }
    }

    fn start_assistant_message(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.current_text.clear();
            state.message_id = Uuid::new_v4().to_string();
        }
    }

    fn send_delta(&self, delta: AssistantDelta) {
        match delta {
            AssistantDelta::Text { text } => {
                let message_id = if let Ok(mut state) = self.state.lock() {
                    state.current_text.push_str(&text);
                    state.message_id.clone()
                } else {
                    return;
                };
                self.send(SessionUpdate::AgentMessageChunk(text_chunk(
                    text,
                    &message_id,
                )));
            }
            AssistantDelta::Reasoning { text } => {
                let message_id = if let Ok(state) = self.state.lock() {
                    state.message_id.clone()
                } else {
                    return;
                };
                self.send(SessionUpdate::AgentThoughtChunk(text_chunk(
                    text,
                    &message_id,
                )));
            }
            AssistantDelta::ToolCallStart { .. } | AssistantDelta::ToolCallArguments { .. } => {}
        }
    }
}

fn text_chunk(text: String, message_id: &str) -> ContentChunk {
    ContentChunk::new(AcpContentBlock::Text(TextContent::new(text)))
        .message_id(MessageId::from(message_id.to_owned()))
}

pub(crate) fn replay_history(
    connection: &ConnectionTo<Client>,
    session_id: &str,
    history: Vec<LocalHistoryEntry>,
) -> Result<(), ServerError> {
    for entry in history {
        let request_id = entry.command_id.to_string();
        for update in replay_message(entry.message, &entry.event_id, &request_id) {
            connection
                .send_notification(SessionNotification::new(session_id.to_owned(), update))
                .map_err(ServerError::Transport)?;
        }
    }
    Ok(())
}

fn replay_message(message: Message, message_id: &str, request_id: &str) -> Vec<SessionUpdate> {
    match message {
        Message::User { content } => content
            .into_iter()
            .map(|block| {
                let mut meta = Meta::new();
                meta.insert(
                    "requestId".to_owned(),
                    serde_json::Value::String(request_id.to_owned()),
                );
                SessionUpdate::UserMessageChunk(
                    ContentChunk::new(acp_content(block))
                        .message_id(MessageId::from(message_id.to_owned()))
                        .meta(meta),
                )
            })
            .collect(),
        Message::Assistant { content, .. } => content
            .into_iter()
            .map(|block| match block {
                AssistantContent::Text { text, .. } => {
                    SessionUpdate::AgentMessageChunk(text_chunk(text, message_id))
                }
                AssistantContent::Reasoning { text, .. } => {
                    SessionUpdate::AgentThoughtChunk(text_chunk(text, message_id))
                }
                AssistantContent::ToolCall { call } => SessionUpdate::ToolCall(
                    AcpToolCall::new(call.id.clone(), tool_title(&call))
                        .kind(tool_kind(&call.name))
                        .status(ToolCallStatus::InProgress)
                        .raw_input(call.arguments),
                ),
            })
            .collect(),
        Message::Tool { result } => vec![SessionUpdate::ToolCallUpdate(tool_result_by_id(
            result.call_id.clone(),
            result,
        ))],
    }
}

impl AgentEventSink for AcpEventSink {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async move { self.observe(event) })
    }
}

fn tool_update(call_id: String, update: ToolOutput) -> ToolCallUpdate {
    ToolCallUpdate::new(
        call_id,
        ToolCallUpdateFields::new()
            .status(ToolCallStatus::InProgress)
            .content(tool_content(update.content))
            .raw_output(update.details),
    )
}

fn tool_result(call: ToolCall, result: ToolResult) -> ToolCallUpdate {
    tool_result_by_id(call.id, result)
}

fn tool_result_by_id(call_id: String, result: ToolResult) -> ToolCallUpdate {
    ToolCallUpdate::new(
        call_id,
        ToolCallUpdateFields::new()
            .status(if result.is_error {
                ToolCallStatus::Failed
            } else {
                ToolCallStatus::Completed
            })
            .content(tool_content(result.content))
            .raw_output(result.details),
    )
}

fn tool_outcome_unknown(
    call_id: String,
    error: &renoa_agent::ToolOutcomeUnknown,
) -> ToolCallUpdate {
    let content = vec![ToolCallContent::from(AcpContentBlock::Text(
        TextContent::new(error.message().to_owned()),
    ))];
    let raw_output = serde_json::json!({
        "error": {
            "code": error.code(),
            "message": error.message(),
            "outcome_unknown": true,
            "partial_changes_possible": error.partial_changes_possible(),
        }
    });
    ToolCallUpdate::new(
        call_id,
        ToolCallUpdateFields::new()
            .status(ToolCallStatus::Failed)
            .content(content)
            .raw_output(raw_output),
    )
}

fn tool_content(content: Vec<ContentBlock>) -> Vec<ToolCallContent> {
    content
        .into_iter()
        .map(|block| ToolCallContent::from(acp_content(block)))
        .collect()
}

fn acp_content(block: ContentBlock) -> AcpContentBlock {
    match block {
        ContentBlock::Text { text } => AcpContentBlock::Text(TextContent::new(text)),
        ContentBlock::Image { data, mime_type } => {
            AcpContentBlock::Image(ImageContent::new(data, mime_type))
        }
    }
}

fn tool_kind(name: &str) -> ToolKind {
    match name {
        "read_file" => ToolKind::Read,
        "edit_file" | "write_file" => ToolKind::Edit,
        "bash" => ToolKind::Execute,
        "grep" | "find" => ToolKind::Search,
        _ => ToolKind::Other,
    }
}

fn tool_title(call: &ToolCall) -> String {
    call.name.replace('_', " ")
}

#[cfg(test)]
mod tests {
    use super::tool_kind;
    use agent_client_protocol::schema::v1::ToolKind;

    #[test]
    fn coding_search_tools_use_the_standard_search_kind() {
        assert_eq!(tool_kind("grep"), ToolKind::Search);
        assert_eq!(tool_kind("find"), ToolKind::Search);
    }
}

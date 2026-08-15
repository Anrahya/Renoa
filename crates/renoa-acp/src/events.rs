use std::sync::Mutex;

use crate::ServerError;
use agent_client_protocol::{
    Client, ConnectionTo,
    schema::v1::{
        ContentBlock as AcpContentBlock, ContentChunk, ImageContent, MessageId, SessionId,
        SessionNotification, SessionUpdate, TextContent, ToolCall as AcpToolCall, ToolCallContent,
        ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
    },
};
use renoa_agent::{
    AgentEvent, AgentEventSink, AssistantDelta, BoxFuture, ContentBlock, MessageRole, ToolCall,
    ToolOutput, ToolResult,
};

pub(crate) struct AcpEventSink {
    connection: ConnectionTo<Client>,
    session_id: SessionId,
    message_id: String,
    state: Mutex<EventState>,
}

#[derive(Default)]
struct EventState {
    current_text: String,
    send_error: Option<String>,
}

impl AcpEventSink {
    pub(crate) fn new(
        connection: ConnectionTo<Client>,
        session_id: impl Into<SessionId>,
        message_id: String,
    ) -> Self {
        Self {
            connection,
            session_id: session_id.into(),
            message_id,
            state: Mutex::new(EventState::default()),
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
        Ok(Some(text_chunk(text.to_owned(), &self.message_id)))
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
            AgentEvent::MessageStart {
                role: MessageRole::Assistant,
            }
            | AgentEvent::MessageAbort => self.clear_text(),
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
            _ => {}
        }
    }

    fn clear_text(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.current_text.clear();
        }
    }

    fn send_delta(&self, delta: AssistantDelta) {
        match delta {
            AssistantDelta::Text { text } => {
                if let Ok(mut state) = self.state.lock() {
                    state.current_text.push_str(&text);
                } else {
                    return;
                }
                self.send(SessionUpdate::AgentMessageChunk(text_chunk(
                    text,
                    &self.message_id,
                )));
            }
            AssistantDelta::Reasoning { text } => {
                self.send(SessionUpdate::AgentThoughtChunk(text_chunk(
                    text,
                    &self.message_id,
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
    ToolCallUpdate::new(
        call.id,
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
        _ => ToolKind::Other,
    }
}

fn tool_title(call: &ToolCall) -> String {
    call.name.replace('_', " ")
}

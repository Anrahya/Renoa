use tokio_util::sync::CancellationToken;

use crate::{
    AgentEvent, AgentEventSink, AssistantContent, ContentBlock, Message, ModelRequest,
    ModelResponse, SamplingError, SamplingResult, StopReason, TokenUsage,
    events::{append_message, emit_event, finish_message},
    sample_model,
};

use super::{Agent, AgentError, AgentRunResult};

impl Agent {
    pub(super) async fn run(
        &mut self,
        prompt: Option<Message>,
        mut pending_input: Vec<Vec<ContentBlock>>,
        cancellation: CancellationToken,
        sink: Option<&dyn AgentEventSink>,
    ) -> Result<AgentRunResult, AgentError> {
        if self.config.max_model_turns == 0 {
            return Err(AgentError::TurnLimit(0));
        }
        if pending_input.is_empty() {
            pending_input = self.control.take_steering(self.config.steering_mode);
        }
        emit_event(sink, AgentEvent::TurnStart).await;
        if let Some(message) = prompt {
            append_message(sink, &mut self.state.messages, message).await;
        }
        self.append_user_input(&mut pending_input, sink).await;
        let mut usage = Some(TokenUsage::default());

        for turn in 0..self.config.max_model_turns {
            if turn > 0 {
                emit_event(sink, AgentEvent::TurnStart).await;
                self.append_user_input(&mut pending_input, sink).await;
            }
            let completed = self.complete_model_turn(&cancellation, sink).await?;
            add_usage(&mut usage, completed.usage);

            if turn + 1 == self.config.max_model_turns {
                if !completed.has_tool_calls
                    && !self.control.has_steering()
                    && !self.control.has_follow_up()
                {
                    return Ok(AgentRunResult {
                        output: completed.output,
                        model_turns: turn + 1,
                        stop_reason: completed.stop_reason,
                        usage,
                    });
                }
                continue;
            }

            pending_input = self.control.take_steering(self.config.steering_mode);
            if !completed.has_tool_calls && pending_input.is_empty() {
                pending_input = self.control.take_follow_up(self.config.follow_up_mode);
                if pending_input.is_empty() {
                    return Ok(AgentRunResult {
                        output: completed.output,
                        model_turns: turn + 1,
                        stop_reason: completed.stop_reason,
                        usage,
                    });
                }
            }
        }

        Err(AgentError::TurnLimit(self.config.max_model_turns))
    }

    async fn append_user_input(
        &mut self,
        pending_input: &mut Vec<Vec<ContentBlock>>,
        sink: Option<&dyn AgentEventSink>,
    ) {
        for content in pending_input.drain(..) {
            append_message(sink, &mut self.state.messages, Message::User { content }).await;
        }
    }

    async fn complete_model_turn(
        &mut self,
        cancellation: &CancellationToken,
        sink: Option<&dyn AgentEventSink>,
    ) -> Result<CompletedTurn, AgentError> {
        let response = match self.stream_model(cancellation, sink).await {
            Ok(response) => response,
            Err(error) => {
                emit_event(sink, AgentEvent::TurnEnd).await;
                return Err(error);
            }
        };

        let ModelResponse {
            content,
            stop_reason,
            usage,
            metadata,
        } = response.response;
        let calls = content
            .iter()
            .filter_map(|block| match block {
                AssistantContent::ToolCall { call } => Some(call.clone()),
                AssistantContent::Text { .. } | AssistantContent::Reasoning { .. } => None,
            })
            .collect::<Vec<_>>();
        if calls.len() > self.config.max_tool_calls_per_turn {
            if response.message_started {
                emit_event(sink, AgentEvent::MessageAbort).await;
            }
            emit_event(sink, AgentEvent::TurnEnd).await;
            return Err(AgentError::ToolCallLimit {
                actual: calls.len(),
                limit: self.config.max_tool_calls_per_turn,
                usage,
            });
        }

        let output = content
            .iter()
            .filter_map(|block| match block {
                AssistantContent::Text { text, .. } => Some(text.as_str()),
                AssistantContent::Reasoning { .. } | AssistantContent::ToolCall { .. } => None,
            })
            .collect::<String>();
        let assistant = Message::Assistant {
            content,
            stop_reason,
            usage,
            metadata,
        };
        finish_message(
            sink,
            &mut self.state.messages,
            assistant,
            response.message_started,
        )
        .await;

        let completed = CompletedTurn {
            output,
            has_tool_calls: !calls.is_empty(),
            stop_reason,
            usage,
        };
        if stop_reason == StopReason::Length {
            self.reject_length_stopped_calls(&calls, sink).await;
        } else {
            self.execute_tool_calls(&calls, cancellation, sink).await?;
        }
        emit_event(sink, AgentEvent::TurnEnd).await;
        Ok(completed)
    }

    async fn stream_model(
        &self,
        cancellation: &CancellationToken,
        sink: Option<&dyn AgentEventSink>,
    ) -> Result<SamplingResult, AgentError> {
        let messages = if let Some(projector) = &self.context_projector {
            let projection =
                projector.project(self.state.messages.clone(), cancellation.child_token());
            tokio::pin!(projection);
            tokio::select! {
                biased;
                () = cancellation.cancelled() => return Err(AgentError::Cancelled),
                messages = &mut projection => messages?,
            }
        } else {
            self.state.messages.clone()
        };
        let request = ModelRequest {
            system_prompt: self.system_prompt.clone(),
            messages,
            tools: self.tools.iter().map(|tool| tool.spec().clone()).collect(),
        };
        sample_model(
            self.model.as_ref(),
            request,
            cancellation.child_token(),
            sink,
        )
        .await
        .map_err(map_sampling_error)
    }
}

fn map_sampling_error(error: SamplingError) -> AgentError {
    match error {
        SamplingError::Cancelled => AgentError::Cancelled,
        SamplingError::Model(error) => AgentError::Model(error),
        SamplingError::IncompleteStream => AgentError::IncompleteModelStream,
    }
}

struct CompletedTurn {
    output: String,
    has_tool_calls: bool,
    stop_reason: StopReason,
    usage: Option<TokenUsage>,
}

fn add_usage(total: &mut Option<TokenUsage>, turn: Option<TokenUsage>) {
    match (total.as_mut(), turn) {
        (Some(total), Some(turn)) => total.add(turn),
        _ => *total = None,
    }
}

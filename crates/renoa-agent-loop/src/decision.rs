use std::sync::Arc;

use renoa_agent::{
    AssistantContent, ContentBlock, Message, ModelRequest, ModelResponse, StopReason, ToolCall,
    ToolResult, ToolSpec, validate_tool_call_ids,
};
use renoa_kernel::{
    EffectOutcome, EffectRecovery, EffectRequest, LoopDecision, LoopError, LoopInput,
    SettledEffectBatch,
};

mod code_mode;
mod compaction;
mod effect_input;
mod interruption;

#[cfg(test)]
mod tests;

use effect_input::{require_effect, require_effect_identity};

use crate::{
    AgentCommand,
    configuration::{AgentLoopConfig, MODEL_EFFECT_BINDING},
    context::{ContextPreparation, ContextStrategy},
    format::{
        AgentCommandKind, LoopPhase, ModelEffectOutput, checkpoint, context_input, message_event,
        message_events, turn_timing_event,
    },
    pending_tools::PendingToolCalls,
};

pub(crate) struct LoopTool {
    pub(crate) spec: ToolSpec,
    pub(crate) effect_binding: String,
    pub(crate) recovery: EffectRecovery,
}

pub(crate) struct LoopCodeMode {
    pub(crate) spec: ToolSpec,
    pub(crate) nested_name: String,
    pub(crate) nested_binding: String,
    pub(crate) nested_recovery: EffectRecovery,
}

pub(crate) struct AgentLoop {
    config: AgentLoopConfig,
    context: Arc<dyn ContextStrategy>,
    model_recovery: EffectRecovery,
    tools: Vec<LoopTool>,
    code_mode: Option<LoopCodeMode>,
}

impl AgentLoop {
    pub(crate) const fn new(
        config: AgentLoopConfig,
        context: Arc<dyn ContextStrategy>,
        model_recovery: EffectRecovery,
        tools: Vec<LoopTool>,
        code_mode: Option<LoopCodeMode>,
    ) -> Self {
        Self {
            config,
            context,
            model_recovery,
            tools,
            code_mode,
        }
    }

    fn decide_initial(&self, input: &LoopInput) -> Result<LoopDecision, LoopError> {
        if input.effect_batch.is_some() {
            return Err(LoopError::new(
                "an uncheckpointed operation cannot have a settled effect",
            ));
        }
        let command = match serde_json::from_value::<AgentCommand>(input.command.content().clone())
        {
            Ok(command) => command,
            Err(error) => {
                return Ok(LoopDecision::Fail {
                    checkpoint: checkpoint(LoopPhase::Terminal)?,
                    events: Vec::new(),
                    reason: format!("invalid agent command: {error}"),
                });
            }
        };
        match command.into_kind() {
            AgentCommandKind::Prompt {
                content,
                turn_timing,
            } => {
                let mut events = vec![message_event(Message::User { content })?];
                if let Some(turn_timing) = turn_timing {
                    events.push(turn_timing_event(turn_timing)?);
                }
                Ok(LoopDecision::AppendEventsAndContinue {
                    checkpoint: checkpoint(LoopPhase::NeedModel { model_turns: 0 })?,
                    events,
                })
            }
            AgentCommandKind::Compact => self.request_explicit_compaction(input),
        }
    }

    fn request_model(
        &self,
        model_turns: u32,
        input: &LoopInput,
    ) -> Result<LoopDecision, LoopError> {
        if input.effect_batch.is_some() {
            return Err(LoopError::new(
                "a model-ready checkpoint cannot have a settled effect",
            ));
        }
        if let Some(limit) = self
            .config
            .max_model_turns
            .filter(|limit| model_turns >= limit.get())
        {
            return Ok(LoopDecision::Fail {
                checkpoint: checkpoint(LoopPhase::Terminal)?,
                events: Vec::new(),
                reason: format!("model exceeded the configured turn limit of {limit}"),
            });
        }
        match self.prepare_context(input.operation_id, &input.events, false)? {
            ContextPreparation::Model { messages } => {
                let next_turn = model_turns
                    .checked_add(1)
                    .ok_or_else(|| LoopError::new("model turn counter overflowed"))?;
                Ok(LoopDecision::InvokeEffects {
                    checkpoint: checkpoint(LoopPhase::AwaitingModel {
                        model_turns: next_turn,
                    })?,
                    effects: vec![EffectRequest {
                        binding: MODEL_EFFECT_BINDING.to_owned(),
                        request: encode("model request", self.model_request(messages))?,
                        recovery: self.model_recovery,
                    }],
                })
            }
            ContextPreparation::Compact { plan, max_attempts } => {
                self.invoke_compaction(model_turns, plan, max_attempts)
            }
            ContextPreparation::CapacityExceeded {
                estimated_input_tokens,
                dispatch_limit_tokens,
            } => {
                Self::context_capacity_failure(estimated_input_tokens, dispatch_limit_tokens, None)
            }
        }
    }

    fn settle_model(&self, model_turns: u32, input: LoopInput) -> Result<LoopDecision, LoopError> {
        let effect = require_effect(input.effect_batch, "model result")?;
        let expected_request = self.normal_model_request(input.operation_id, &input.events)?;
        require_effect_identity(
            &effect,
            MODEL_EFFECT_BINDING,
            &encode("model request", expected_request)?,
        )?;
        let output = match effect.outcome {
            EffectOutcome::Success(output) => decode("model effect output", output)?,
            EffectOutcome::Failure { message } => {
                return Ok(LoopDecision::Fail {
                    checkpoint: checkpoint(LoopPhase::Terminal)?,
                    events: Vec::new(),
                    reason: message,
                });
            }
            _ => {
                return Err(LoopError::new(
                    "model effect outcome version is unsupported",
                ));
            }
        };
        match output {
            ModelEffectOutput::Completed { response } => {
                self.classify_model_response(model_turns, response)
            }
            ModelEffectOutput::ContextWindowExceeded { message } => self
                .compact_after_provider_overflow(
                    model_turns,
                    input.operation_id,
                    &input.events,
                    &message,
                ),
        }
    }

    fn classify_model_response(
        &self,
        model_turns: u32,
        response: ModelResponse,
    ) -> Result<LoopDecision, LoopError> {
        let calls = response
            .content
            .iter()
            .filter_map(|content| match content {
                AssistantContent::ToolCall { call } => Some(call.clone()),
                AssistantContent::Text { .. } | AssistantContent::Reasoning { .. } => None,
            })
            .collect::<Vec<_>>();
        let too_many_calls = u32::try_from(calls.len()).map_or(true, |count| {
            count > self.config.max_tool_calls_per_turn.get()
        });
        if too_many_calls {
            return Ok(LoopDecision::Fail {
                checkpoint: checkpoint(LoopPhase::Terminal)?,
                events: Vec::new(),
                reason: format!(
                    "model returned {} tool calls; the per-turn limit is {}",
                    calls.len(),
                    self.config.max_tool_calls_per_turn
                ),
            });
        }
        if let Err(error) = validate_tool_call_ids(calls.iter().map(|call| call.id.as_str())) {
            return Ok(LoopDecision::Fail {
                checkpoint: checkpoint(LoopPhase::Terminal)?,
                events: Vec::new(),
                reason: error.to_string(),
            });
        }
        let stop_reason = response.stop_reason;
        let assistant = Message::Assistant {
            content: response.content,
            stop_reason,
            usage: response.usage,
            metadata: response.metadata,
        };
        if calls.is_empty() {
            return Ok(LoopDecision::Complete {
                checkpoint: checkpoint(LoopPhase::Terminal)?,
                events: vec![message_event(assistant)?],
            });
        }
        if stop_reason == StopReason::Length {
            let mut messages = Vec::with_capacity(calls.len() + 1);
            messages.push(assistant);
            messages.extend(calls.iter().map(|call| {
                Message::Tool {
                    result: unavailable_result(
                        call,
                        "Tool call was not executed because the model response reached its token limit.",
                    ),
                }
            }));
            return Ok(LoopDecision::AppendEventsAndContinue {
                checkpoint: checkpoint(LoopPhase::NeedModel { model_turns })?,
                events: message_events(messages)?,
            });
        }
        Ok(LoopDecision::AppendEventsAndContinue {
            checkpoint: checkpoint(LoopPhase::NeedTool {
                model_turns,
                pending: PendingToolCalls::new(calls)?,
            })?,
            events: vec![message_event(assistant)?],
        })
    }

    fn prepare_context(
        &self,
        active_operation_id: renoa_kernel::OperationId,
        events: &[renoa_kernel::SemanticEvent],
        compaction_required: bool,
    ) -> Result<ContextPreparation, LoopError> {
        let preparation = self
            .context
            .prepare(self.build_context_input(active_operation_id, events, compaction_required)?)
            .map_err(|error| LoopError::new(format!("context projection failed: {error}")))?;
        if let ContextPreparation::Compact { plan, .. } = &preparation {
            self.validate_compaction_plan(active_operation_id, events, plan)?;
        }
        Ok(preparation)
    }

    fn build_context_input(
        &self,
        active_operation_id: renoa_kernel::OperationId,
        events: &[renoa_kernel::SemanticEvent],
        compaction_required: bool,
    ) -> Result<crate::ContextInput, LoopError> {
        let tools = self
            .tools
            .iter()
            .map(|tool| tool.spec.clone())
            .chain(self.code_mode.iter().map(|code| code.spec.clone()))
            .collect::<Vec<_>>();
        context_input(
            active_operation_id,
            events,
            &self.config.system_prompt,
            &tools,
            compaction_required,
        )
    }

    pub(super) fn validate_compaction_plan(
        &self,
        active_operation_id: renoa_kernel::OperationId,
        events: &[renoa_kernel::SemanticEvent],
        plan: &crate::CompactionPlan,
    ) -> Result<(), LoopError> {
        let input = self.build_context_input(active_operation_id, events, false)?;
        crate::compaction::validate_plan(&input, plan)
            .map_err(|error| LoopError::new(format!("context compaction plan is invalid: {error}")))
    }

    pub(super) fn validate_explicit_compaction_plan(
        input: &crate::ContextInput,
        plan: &crate::CompactionPlan,
    ) -> Result<(), LoopError> {
        crate::compaction::validate_explicit_plan(input, plan).map_err(|error| {
            LoopError::new(format!(
                "explicit context compaction plan is invalid: {error}"
            ))
        })
    }

    fn normal_model_request(
        &self,
        active_operation_id: renoa_kernel::OperationId,
        events: &[renoa_kernel::SemanticEvent],
    ) -> Result<ModelRequest, LoopError> {
        match self.prepare_context(active_operation_id, events, false)? {
            ContextPreparation::Model { messages } => Ok(self.model_request(messages)),
            ContextPreparation::Compact { .. } => Err(LoopError::new(
                "context strategy changed a persisted model request into a compaction request",
            )),
            ContextPreparation::CapacityExceeded { .. } => Err(LoopError::new(
                "context strategy changed a persisted model request into a capacity failure",
            )),
        }
    }

    fn model_request(&self, messages: Vec<Message>) -> ModelRequest {
        ModelRequest {
            system_prompt: self.config.system_prompt.clone(),
            messages: crate::context::model_visible_messages(messages),
            tools: self
                .tools
                .iter()
                .map(|tool| tool.spec.clone())
                .chain(self.code_mode.iter().map(|code| code.spec.clone()))
                .collect(),
        }
    }

    fn request_tool(
        &self,
        model_turns: u32,
        pending: PendingToolCalls,
        input: &LoopInput,
    ) -> Result<LoopDecision, LoopError> {
        if input.effect_batch.is_some() {
            return Err(LoopError::new(
                "a tool-ready checkpoint cannot have a settled effect",
            ));
        }
        let call = pending.current();
        if self
            .code_mode
            .as_ref()
            .is_some_and(|code| call.name == code.spec.name)
        {
            return Self::request_code_mode(model_turns, pending, input.operation_id);
        }
        let Some(tool) = self.tools.iter().find(|tool| tool.spec.name == call.name) else {
            let result =
                unavailable_result(call, &format!("Tool `{}` is not available.", call.name));
            return Self::append_tool_result(model_turns, pending, result);
        };
        let request = encode("tool request", call)?;
        Ok(LoopDecision::InvokeEffects {
            checkpoint: checkpoint(LoopPhase::AwaitingTool {
                model_turns,
                pending,
            })?,
            effects: vec![EffectRequest {
                binding: tool.effect_binding.clone(),
                request,
                recovery: tool.recovery,
            }],
        })
    }

    fn settle_tool(
        &self,
        model_turns: u32,
        pending: PendingToolCalls,
        effect_batch: Option<SettledEffectBatch>,
    ) -> Result<LoopDecision, LoopError> {
        let call = pending.current();
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.spec.name == call.name)
            .ok_or_else(|| LoopError::new("awaited tool binding is no longer configured"))?;
        let effect = require_effect(effect_batch, "tool result")?;
        require_effect_identity(
            &effect,
            &tool.effect_binding,
            &encode("tool request", call)?,
        )?;
        let result = match effect.outcome {
            EffectOutcome::Success(result) => decode::<ToolResult>("tool result", result)?,
            EffectOutcome::Failure { message } => {
                return Self::fail_tool(&pending, message);
            }
            _ => return Err(LoopError::new("tool effect outcome version is unsupported")),
        };
        if result.call_id != call.id || result.name != call.name {
            return Err(LoopError::new(
                "tool result identity differs from its persisted request",
            ));
        }
        Self::append_tool_result(model_turns, pending, result)
    }

    fn append_tool_result(
        model_turns: u32,
        pending: PendingToolCalls,
        result: ToolResult,
    ) -> Result<LoopDecision, LoopError> {
        let phase = pending
            .advance()
            .map_or(LoopPhase::NeedModel { model_turns }, |pending| {
                LoopPhase::NeedTool {
                    model_turns,
                    pending,
                }
            });
        Ok(LoopDecision::AppendEventsAndContinue {
            checkpoint: checkpoint(phase)?,
            events: vec![message_event(Message::Tool { result })?],
        })
    }

    fn fail_tool(pending: &PendingToolCalls, reason: String) -> Result<LoopDecision, LoopError> {
        let events = pending
            .iter()
            .enumerate()
            .map(|(position, call)| Message::Tool {
                result: unavailable_result(
                    call,
                    if position == 0 {
                        "Tool execution ended without a model-visible result."
                    } else {
                        "Tool call was not run because an earlier tool failed."
                    },
                ),
            });
        Ok(LoopDecision::Fail {
            checkpoint: checkpoint(LoopPhase::Terminal)?,
            events: message_events(events)?,
            reason,
        })
    }
}

fn encode<T: serde::Serialize>(
    description: &str,
    value: T,
) -> Result<serde_json::Value, LoopError> {
    serde_json::to_value(value)
        .map_err(|error| LoopError::new(format!("{description} encoding failed: {error}")))
}

fn decode<T: serde::de::DeserializeOwned>(
    description: &str,
    value: serde_json::Value,
) -> Result<T, LoopError> {
    serde_json::from_value(value)
        .map_err(|error| LoopError::new(format!("{description} is invalid: {error}")))
}

fn unavailable_result(call: &ToolCall, message: &str) -> ToolResult {
    ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: vec![ContentBlock::text(message)],
        details: None,
        is_error: true,
    }
}

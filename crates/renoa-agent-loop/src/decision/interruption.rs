use renoa_agent::{Message, ToolCall, ToolResult};
use renoa_kernel::{
    CancellationEffect, CancellationInput, CancellationTransition, EffectOutcome, LoopDecision,
    LoopError, LoopInput, LoopPlugin, UnknownEffect, UnknownEffectAbandonment, UnknownEffectInput,
};

use super::{
    AgentLoop, LoopPhase, MODEL_EFFECT_BINDING, checkpoint, decode, encode,
    require_effect_request_identity, unavailable_result,
};
use crate::format::{decode_checkpoint, message_events};

impl LoopPlugin for AgentLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        let Some(saved) = input.checkpoint.as_ref() else {
            return self.decide_initial(&input);
        };
        match decode_checkpoint(saved)? {
            LoopPhase::NeedModel { model_turns } => self.request_model(model_turns, &input),
            LoopPhase::AwaitingModel { model_turns } => self.settle_model(model_turns, input),
            LoopPhase::AwaitingCompaction {
                model_turns,
                plan,
                max_attempts,
                attempt,
            } => self.settle_compaction(model_turns, plan, max_attempts, attempt, input),
            LoopPhase::AwaitingExplicitCompaction {
                plan,
                max_attempts,
                attempt,
            } => self.settle_explicit_compaction(plan, max_attempts, attempt, input),
            LoopPhase::NeedTool {
                model_turns,
                calls,
                next_index,
            } => self.request_tool(model_turns, calls, next_index, input.effect.as_ref()),
            LoopPhase::AwaitingTool {
                model_turns,
                calls,
                next_index,
            } => self.settle_tool(model_turns, calls, next_index, input.effect),
            LoopPhase::Terminal => Err(LoopError::new(
                "a terminal agent checkpoint cannot be driven",
            )),
        }
    }

    fn abandon_unknown_effect(
        &self,
        input: UnknownEffectInput,
    ) -> Result<UnknownEffectAbandonment, LoopError> {
        match decode_checkpoint(&input.checkpoint)? {
            LoopPhase::AwaitingModel { .. } => self.abandon_unknown_model(&input),
            LoopPhase::AwaitingCompaction { plan, .. }
            | LoopPhase::AwaitingExplicitCompaction { plan, .. } => {
                Self::abandon_unknown_compaction(&plan, &input)
            }
            LoopPhase::AwaitingTool {
                calls, next_index, ..
            } => self.abandon_unknown_tool(&calls, next_index, &input),
            LoopPhase::NeedModel { .. } | LoopPhase::NeedTool { .. } | LoopPhase::Terminal => Err(
                LoopError::new("checkpoint is not awaiting the unknown effect"),
            ),
        }
    }

    fn cancel_operation(
        &self,
        input: CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        let Some(saved) = input.checkpoint.as_ref() else {
            require_no_cancellation_effect(&input)?;
            return cancelled(Vec::new());
        };
        match decode_checkpoint(saved)? {
            LoopPhase::NeedModel { .. } => {
                require_no_cancellation_effect(&input)?;
                cancelled(Vec::new())
            }
            LoopPhase::AwaitingModel { .. } => self.cancel_model(&input),
            LoopPhase::AwaitingCompaction { plan, .. }
            | LoopPhase::AwaitingExplicitCompaction { plan, .. } => {
                Self::cancel_compaction(&plan, &input)
            }
            LoopPhase::NeedTool {
                calls, next_index, ..
            } => Self::cancel_planned_tools(&calls, next_index, &input),
            LoopPhase::AwaitingTool {
                calls, next_index, ..
            } => self.cancel_current_tool(&calls, next_index, &input),
            LoopPhase::Terminal => Err(LoopError::new(
                "a terminal agent checkpoint cannot be cancelled",
            )),
        }
    }
}

impl AgentLoop {
    fn abandon_unknown_model(
        &self,
        input: &UnknownEffectInput,
    ) -> Result<UnknownEffectAbandonment, LoopError> {
        let expected_request = self.normal_model_request(input.operation_id, &input.events)?;
        require_unknown_effect_identity(
            &input.effect,
            MODEL_EFFECT_BINDING,
            &encode("model request", expected_request)?,
        )?;
        Ok(UnknownEffectAbandonment {
            checkpoint: checkpoint(LoopPhase::Terminal)?,
            events: Vec::new(),
        })
    }

    fn abandon_unknown_compaction(
        plan: &crate::CompactionPlan,
        input: &UnknownEffectInput,
    ) -> Result<UnknownEffectAbandonment, LoopError> {
        require_unknown_effect_identity(
            &input.effect,
            MODEL_EFFECT_BINDING,
            &encode("compaction request", plan.summary_request())?,
        )?;
        Ok(UnknownEffectAbandonment {
            checkpoint: checkpoint(LoopPhase::Terminal)?,
            events: Vec::new(),
        })
    }

    fn abandon_unknown_tool(
        &self,
        calls: &[ToolCall],
        next_index: u32,
        input: &UnknownEffectInput,
    ) -> Result<UnknownEffectAbandonment, LoopError> {
        let (index, call) = indexed_call(calls, next_index)?;
        let tool = self.configured_tool(call)?;
        require_unknown_effect_identity(
            &input.effect,
            &tool.effect_binding,
            &encode("tool request", call)?,
        )?;
        let mut results = Vec::with_capacity(calls.len() - index);
        results.push(Message::Tool {
            result: unavailable_result(
                call,
                "This tool may have finished, but Renoa could not recover its result. It was not run again.",
            ),
        });
        results.extend(calls[index + 1..].iter().map(|call| Message::Tool {
            result: unavailable_result(
                call,
                "Tool call was not run because an earlier tool outcome is unknown.",
            ),
        }));
        Ok(UnknownEffectAbandonment {
            checkpoint: checkpoint(LoopPhase::Terminal)?,
            events: message_events(results)?,
        })
    }

    fn cancel_model(&self, input: &CancellationInput) -> Result<CancellationTransition, LoopError> {
        let expected_request = self.normal_model_request(input.operation_id, &input.events)?;
        let effect = input.effect.as_ref().ok_or_else(|| {
            LoopError::new("an awaiting model checkpoint has no cancellation effect")
        })?;
        require_cancellation_effect_identity(
            effect,
            MODEL_EFFECT_BINDING,
            &encode("model request", expected_request)?,
        )?;
        cancelled(Vec::new())
    }

    fn cancel_compaction(
        plan: &crate::CompactionPlan,
        input: &CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        let effect = input.effect.as_ref().ok_or_else(|| {
            LoopError::new("an awaiting compaction checkpoint has no cancellation effect")
        })?;
        require_cancellation_effect_identity(
            effect,
            MODEL_EFFECT_BINDING,
            &encode("compaction request", plan.summary_request())?,
        )?;
        cancelled(Vec::new())
    }

    fn cancel_planned_tools(
        calls: &[ToolCall],
        next_index: u32,
        input: &CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        require_no_cancellation_effect(input)?;
        let (index, _) = indexed_call(calls, next_index)?;
        let results = calls[index..].iter().map(|call| Message::Tool {
            result: unavailable_result(
                call,
                "Tool call was not run because the operation was cancelled.",
            ),
        });
        cancelled(message_events(results)?)
    }

    fn cancel_current_tool(
        &self,
        calls: &[ToolCall],
        next_index: u32,
        input: &CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        let (index, call) = indexed_call(calls, next_index)?;
        let tool = self.configured_tool(call)?;
        let effect = input.effect.as_ref().ok_or_else(|| {
            LoopError::new("an awaiting tool checkpoint has no cancellation effect")
        })?;
        require_cancellation_effect_identity(
            effect,
            &tool.effect_binding,
            &encode("tool request", call)?,
        )?;
        let current = match effect {
            CancellationEffect::NotDispatched(_) => unavailable_result(
                call,
                "Tool call was not run because the operation was cancelled.",
            ),
            CancellationEffect::OutcomeUnknown(_) => unavailable_result(
                call,
                "This tool may have finished, but its result is unavailable because the operation was cancelled.",
            ),
            CancellationEffect::Settled(effect) => settled_tool_result(call, &effect.outcome)?,
            _ => return Err(LoopError::new("cancellation effect version is unsupported")),
        };
        let mut results = Vec::with_capacity(calls.len() - index);
        results.push(Message::Tool { result: current });
        results.extend(calls[index + 1..].iter().map(|call| Message::Tool {
            result: unavailable_result(
                call,
                "Tool call was not run because the operation was cancelled.",
            ),
        }));
        cancelled(message_events(results)?)
    }

    fn configured_tool(&self, call: &ToolCall) -> Result<&super::LoopTool, LoopError> {
        self.tools
            .iter()
            .find(|tool| tool.spec.name == call.name)
            .ok_or_else(|| LoopError::new("awaited tool binding is no longer configured"))
    }
}

fn settled_tool_result(call: &ToolCall, outcome: &EffectOutcome) -> Result<ToolResult, LoopError> {
    match outcome {
        EffectOutcome::Success(value) => {
            let result = decode::<ToolResult>("tool result", value.clone())?;
            if result.call_id != call.id || result.name != call.name {
                return Err(LoopError::new(
                    "tool result identity differs from its persisted request",
                ));
            }
            Ok(result)
        }
        EffectOutcome::Failure { .. } => Ok(unavailable_result(
            call,
            "Tool execution ended without a model-visible result before the operation was cancelled.",
        )),
        _ => Err(LoopError::new("tool effect outcome version is unsupported")),
    }
}

fn indexed_call(calls: &[ToolCall], next_index: u32) -> Result<(usize, &ToolCall), LoopError> {
    let index = usize::try_from(next_index)
        .map_err(|error| LoopError::new(format!("tool call index is invalid: {error}")))?;
    let call = calls
        .get(index)
        .ok_or_else(|| LoopError::new("tool checkpoint points outside its durable call batch"))?;
    Ok((index, call))
}

fn require_no_cancellation_effect(input: &CancellationInput) -> Result<(), LoopError> {
    if input.effect.is_some() {
        Err(LoopError::new(
            "a ready checkpoint cannot contain a cancellation effect",
        ))
    } else {
        Ok(())
    }
}

fn require_unknown_effect_identity(
    effect: &UnknownEffect,
    binding: &str,
    request: &serde_json::Value,
) -> Result<(), LoopError> {
    require_effect_request_identity(
        "unknown",
        &effect.binding,
        &effect.request,
        binding,
        request,
    )
}

fn require_cancellation_effect_identity(
    effect: &CancellationEffect,
    binding: &str,
    request: &serde_json::Value,
) -> Result<(), LoopError> {
    let (kind, actual_binding, actual_request) = match effect {
        CancellationEffect::NotDispatched(effect) => {
            ("not-dispatched", effect.binding.as_str(), &effect.request)
        }
        CancellationEffect::Settled(effect) => {
            ("settled", effect.binding.as_str(), &effect.request)
        }
        CancellationEffect::OutcomeUnknown(effect) => {
            ("unknown", effect.binding.as_str(), &effect.request)
        }
        _ => return Err(LoopError::new("cancellation effect version is unsupported")),
    };
    require_effect_request_identity(kind, actual_binding, actual_request, binding, request)
}

fn cancelled(events: Vec<renoa_kernel::NewEvent>) -> Result<CancellationTransition, LoopError> {
    Ok(CancellationTransition {
        checkpoint: checkpoint(LoopPhase::Terminal)?,
        events,
    })
}

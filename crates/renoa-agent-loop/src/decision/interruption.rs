use renoa_agent::{Message, ToolCall, ToolResult};
use renoa_kernel::{
    CancellationInput, CancellationTransition, EffectBatchFacts, EffectFact, EffectOutcome,
    LoopDecision, LoopError, LoopInput, LoopPlugin, UnknownEffectAbandonment, UnknownEffectInput,
};

use super::effect_input::require_effect_request_identity;
use super::{
    AgentLoop, LoopPhase, MODEL_EFFECT_BINDING, checkpoint, decode, encode, unavailable_result,
};
use crate::format::{decode_checkpoint, message_events};
use crate::pending_tools::PendingToolCalls;

impl LoopPlugin for AgentLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        let Some(saved) = input.checkpoint.as_ref() else {
            return self.decide_initial(&input);
        };
        let phase = decode_checkpoint(saved)?;
        self.validate_code_phase(&phase, input.operation_id)?;
        match phase {
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
                pending,
            } => self.request_tool(model_turns, pending, &input),
            LoopPhase::AwaitingTool {
                model_turns,
                pending,
            } => self.settle_tool(model_turns, pending, input.effect_batch),
            LoopPhase::AwaitingCodeStep {
                model_turns,
                pending,
                run,
                request,
            } => self.settle_code_step(model_turns, pending, run, &request, input.effect_batch),
            LoopPhase::AwaitingCodeCalls {
                model_turns,
                pending,
                run,
                snapshot,
                calls,
            } => self.settle_code_calls(
                model_turns,
                pending,
                run,
                snapshot,
                &calls,
                input.effect_batch,
            ),
            LoopPhase::Terminal => Err(LoopError::new(
                "a terminal agent checkpoint cannot be driven",
            )),
        }
    }

    fn abandon_unknown_effect(
        &self,
        input: UnknownEffectInput,
    ) -> Result<UnknownEffectAbandonment, LoopError> {
        let phase = decode_checkpoint(&input.checkpoint)?;
        self.validate_code_phase(&phase, input.operation_id)?;
        match phase {
            LoopPhase::AwaitingModel { .. } => self.abandon_unknown_model(&input),
            LoopPhase::AwaitingCompaction { plan, .. }
            | LoopPhase::AwaitingExplicitCompaction { plan, .. } => {
                Self::abandon_unknown_compaction(&plan, &input)
            }
            LoopPhase::AwaitingTool { pending, .. } => self.abandon_unknown_tool(&pending, &input),
            LoopPhase::AwaitingCodeStep {
                pending, request, ..
            } => self.abandon_unknown_code_step(&pending, &request, &input),
            LoopPhase::AwaitingCodeCalls {
                pending,
                run,
                calls,
                ..
            } => self.abandon_unknown_code_calls(&pending, &run, &calls, &input),
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
        let phase = decode_checkpoint(saved)?;
        self.validate_code_phase(&phase, input.operation_id)?;
        match phase {
            LoopPhase::NeedModel { .. } => {
                require_no_cancellation_effect(&input)?;
                cancelled(Vec::new())
            }
            LoopPhase::AwaitingModel { .. } => self.cancel_model(&input),
            LoopPhase::AwaitingCompaction { plan, .. }
            | LoopPhase::AwaitingExplicitCompaction { plan, .. } => {
                Self::cancel_compaction(&plan, &input)
            }
            LoopPhase::NeedTool { pending, .. } => Self::cancel_planned_tools(&pending, &input),
            LoopPhase::AwaitingTool { pending, .. } => self.cancel_current_tool(&pending, &input),
            LoopPhase::AwaitingCodeStep {
                pending, request, ..
            } => self.cancel_code_step(&pending, &request, &input),
            LoopPhase::AwaitingCodeCalls {
                pending,
                run,
                calls,
                ..
            } => self.cancel_code_calls(&pending, &run, &calls, &input),
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
            &input.effect_batch,
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
            &input.effect_batch,
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
        pending: &PendingToolCalls,
        input: &UnknownEffectInput,
    ) -> Result<UnknownEffectAbandonment, LoopError> {
        let call = pending.current();
        let tool = self.configured_tool(call)?;
        require_unknown_effect_identity(
            &input.effect_batch,
            &tool.effect_binding,
            &encode("tool request", call)?,
        )?;
        let results = pending.iter().enumerate().map(|(position, call)| Message::Tool {
            result: unavailable_result(
                call,
                if position == 0 {
                    "This tool may have finished, but Renoa could not recover a definite result."
                } else {
                    "Tool call was not run because an earlier tool outcome is unknown."
                },
            ),
        });
        Ok(UnknownEffectAbandonment {
            checkpoint: checkpoint(LoopPhase::Terminal)?,
            events: message_events(results)?,
        })
    }

    fn cancel_model(&self, input: &CancellationInput) -> Result<CancellationTransition, LoopError> {
        let expected_request = self.normal_model_request(input.operation_id, &input.events)?;
        let effect_batch = input.effect_batch.as_ref().ok_or_else(|| {
            LoopError::new("an awaiting model checkpoint has no cancellation effect batch")
        })?;
        require_cancellation_effect_identity(
            effect_batch,
            MODEL_EFFECT_BINDING,
            &encode("model request", expected_request)?,
        )?;
        cancelled(Vec::new())
    }

    fn cancel_compaction(
        plan: &crate::CompactionPlan,
        input: &CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        let effect_batch = input.effect_batch.as_ref().ok_or_else(|| {
            LoopError::new("an awaiting compaction checkpoint has no cancellation effect batch")
        })?;
        require_cancellation_effect_identity(
            effect_batch,
            MODEL_EFFECT_BINDING,
            &encode("compaction request", plan.summary_request())?,
        )?;
        cancelled(Vec::new())
    }

    fn cancel_planned_tools(
        pending: &PendingToolCalls,
        input: &CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        require_no_cancellation_effect(input)?;
        let results = pending.iter().map(|call| Message::Tool {
            result: unavailable_result(
                call,
                "Tool call was not run because the operation was cancelled.",
            ),
        });
        cancelled(message_events(results)?)
    }

    fn cancel_current_tool(
        &self,
        pending: &PendingToolCalls,
        input: &CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        let call = pending.current();
        let tool = self.configured_tool(call)?;
        let effect_batch = input.effect_batch.as_ref().ok_or_else(|| {
            LoopError::new("an awaiting tool checkpoint has no cancellation effect batch")
        })?;
        let effect = require_cancellation_effect_identity(
            effect_batch,
            &tool.effect_binding,
            &encode("tool request", call)?,
        )?;
        let current = match effect {
            EffectFact::NotDispatched(_) => unavailable_result(
                call,
                "Tool call was not run because the operation was cancelled.",
            ),
            EffectFact::OutcomeUnknown(_) => unavailable_result(
                call,
                "This tool may have finished, but its result is unavailable because the operation was cancelled.",
            ),
            EffectFact::Settled(effect) => settled_tool_result(call, &effect.outcome)?,
            _ => return Err(LoopError::new("cancellation effect version is unsupported")),
        };
        let results = std::iter::once(Message::Tool { result: current }).chain(
            pending.remaining().map(|call| Message::Tool {
                result: unavailable_result(
                    call,
                    "Tool call was not run because the operation was cancelled.",
                ),
            }),
        );
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

fn require_no_cancellation_effect(input: &CancellationInput) -> Result<(), LoopError> {
    if input.effect_batch.is_some() {
        Err(LoopError::new(
            "a ready checkpoint cannot contain a cancellation effect",
        ))
    } else {
        Ok(())
    }
}

pub(super) fn require_unknown_effect_identity(
    batch: &EffectBatchFacts,
    binding: &str,
    request: &serde_json::Value,
) -> Result<(), LoopError> {
    let effect = require_single_effect_fact(batch, "unknown")?;
    let EffectFact::OutcomeUnknown(effect) = effect else {
        return Err(LoopError::new(
            "unknown effect batch contains a child that is not unknown",
        ));
    };
    require_effect_request_identity(
        "unknown",
        &effect.binding,
        &effect.request,
        binding,
        request,
    )
}

pub(super) fn require_cancellation_effect_identity<'a>(
    batch: &'a EffectBatchFacts,
    binding: &str,
    request: &serde_json::Value,
) -> Result<&'a EffectFact, LoopError> {
    let effect = require_single_effect_fact(batch, "cancellation")?;
    let (kind, actual_binding, actual_request) = match effect {
        EffectFact::NotDispatched(effect) => {
            ("not-dispatched", effect.binding.as_str(), &effect.request)
        }
        EffectFact::Settled(effect) => ("settled", effect.binding.as_str(), &effect.request),
        EffectFact::OutcomeUnknown(effect) => ("unknown", effect.binding.as_str(), &effect.request),
        _ => return Err(LoopError::new("cancellation effect version is unsupported")),
    };
    require_effect_request_identity(kind, actual_binding, actual_request, binding, request)?;
    Ok(effect)
}

fn require_single_effect_fact<'a>(
    batch: &'a EffectBatchFacts,
    expected: &str,
) -> Result<&'a EffectFact, LoopError> {
    let [effect] = batch.effects.as_slice() else {
        return Err(LoopError::new(format!(
            "{expected} effect batch must contain exactly one effect"
        )));
    };
    Ok(effect)
}

pub(super) fn cancelled(
    events: Vec<renoa_kernel::NewEvent>,
) -> Result<CancellationTransition, LoopError> {
    Ok(CancellationTransition {
        checkpoint: checkpoint(LoopPhase::Terminal)?,
        events,
    })
}

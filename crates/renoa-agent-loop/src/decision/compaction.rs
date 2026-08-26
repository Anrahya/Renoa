use std::num::NonZeroU32;

use renoa_kernel::{EffectOutcome, LoopDecision, LoopError, LoopInput};

use super::{
    AgentLoop, LoopPhase, MODEL_EFFECT_BINDING, checkpoint, decode, encode, require_effect,
    require_effect_identity,
};
use crate::{
    CompactionPlan,
    context::{ContextPreparation, ExplicitCompactionPreparation},
    format::{ModelEffectOutput, compaction_result_event, context_checkpoint_event},
};

#[derive(Clone, Copy)]
enum CompactionContinuation {
    Model { model_turns: u32 },
    Explicit,
}

impl AgentLoop {
    pub(super) fn invoke_compaction(
        &self,
        model_turns: u32,
        plan: CompactionPlan,
        max_attempts: NonZeroU32,
    ) -> Result<LoopDecision, LoopError> {
        self.invoke_compaction_attempt(
            CompactionContinuation::Model { model_turns },
            plan,
            max_attempts,
            NonZeroU32::MIN,
        )
    }

    fn invoke_compaction_attempt(
        &self,
        continuation: CompactionContinuation,
        plan: CompactionPlan,
        max_attempts: NonZeroU32,
        attempt: NonZeroU32,
    ) -> Result<LoopDecision, LoopError> {
        let request = encode("compaction request", plan.summary_request())?;
        let phase = match continuation {
            CompactionContinuation::Model { model_turns } => LoopPhase::AwaitingCompaction {
                model_turns,
                plan,
                max_attempts,
                attempt,
            },
            CompactionContinuation::Explicit => LoopPhase::AwaitingExplicitCompaction {
                plan,
                max_attempts,
                attempt,
            },
        };
        Ok(LoopDecision::InvokeEffect {
            checkpoint: checkpoint(phase)?,
            binding: MODEL_EFFECT_BINDING.to_owned(),
            request,
            recovery: self.model_recovery,
        })
    }

    pub(super) fn request_explicit_compaction(
        &self,
        input: &LoopInput,
    ) -> Result<LoopDecision, LoopError> {
        if input.effect.is_some() {
            return Err(LoopError::new(
                "an explicit-compaction-ready checkpoint cannot have a settled effect",
            ));
        }
        let context = self.build_context_input(input.operation_id, &input.events, false)?;
        let preparation = self
            .context
            .prepare_explicit_compaction(&context)
            .map_err(|error| {
                LoopError::new(format!("explicit context compaction failed: {error}"))
            })?;
        match preparation {
            ExplicitCompactionPreparation::UpToDate {
                estimated_input_tokens,
            } => Ok(LoopDecision::Complete {
                checkpoint: checkpoint(LoopPhase::Terminal)?,
                events: vec![compaction_result_event(estimated_input_tokens)?],
            }),
            ExplicitCompactionPreparation::Compact { plan, max_attempts } => {
                Self::validate_explicit_compaction_plan(&context, &plan)?;
                self.invoke_compaction_attempt(
                    CompactionContinuation::Explicit,
                    plan,
                    max_attempts,
                    NonZeroU32::MIN,
                )
            }
            ExplicitCompactionPreparation::CapacityExceeded {
                estimated_input_tokens,
                dispatch_limit_tokens,
            } => {
                Self::context_capacity_failure(estimated_input_tokens, dispatch_limit_tokens, None)
            }
        }
    }

    pub(super) fn settle_compaction(
        &self,
        model_turns: u32,
        plan: CompactionPlan,
        max_attempts: NonZeroU32,
        attempt: NonZeroU32,
        input: LoopInput,
    ) -> Result<LoopDecision, LoopError> {
        self.settle_compaction_for(
            CompactionContinuation::Model { model_turns },
            plan,
            max_attempts,
            attempt,
            input,
        )
    }

    pub(super) fn settle_explicit_compaction(
        &self,
        plan: CompactionPlan,
        max_attempts: NonZeroU32,
        attempt: NonZeroU32,
        input: LoopInput,
    ) -> Result<LoopDecision, LoopError> {
        self.settle_compaction_for(
            CompactionContinuation::Explicit,
            plan,
            max_attempts,
            attempt,
            input,
        )
    }

    fn settle_compaction_for(
        &self,
        continuation: CompactionContinuation,
        plan: CompactionPlan,
        max_attempts: NonZeroU32,
        attempt: NonZeroU32,
        input: LoopInput,
    ) -> Result<LoopDecision, LoopError> {
        let explicit_context = match continuation {
            CompactionContinuation::Model { .. } => {
                self.validate_compaction_plan(input.operation_id, &input.events, &plan)?;
                None
            }
            CompactionContinuation::Explicit => {
                let context = self.build_context_input(input.operation_id, &input.events, false)?;
                Self::validate_explicit_compaction_plan(&context, &plan)?;
                Some(context)
            }
        };
        let effect = require_effect(input.effect, "compaction result")?;
        require_effect_identity(
            &effect,
            MODEL_EFFECT_BINDING,
            &encode("compaction request", plan.summary_request())?,
        )?;
        let output = match effect.outcome {
            EffectOutcome::Success(output) => decode("compaction model output", output)?,
            EffectOutcome::Failure { message } => {
                return Self::compaction_failure(message);
            }
            _ => {
                return Err(LoopError::new(
                    "compaction effect outcome version is unsupported",
                ));
            }
        };
        match output {
            ModelEffectOutput::Completed { response } => {
                let tools = self
                    .tools
                    .iter()
                    .map(|tool| tool.spec.clone())
                    .collect::<Vec<_>>();
                match self.context.validate_compaction(
                    &plan,
                    &response,
                    &self.config.system_prompt,
                    &tools,
                ) {
                    Ok(summary) => match continuation {
                        CompactionContinuation::Model { model_turns } => {
                            Ok(LoopDecision::AppendEventsAndContinue {
                                checkpoint: checkpoint(LoopPhase::NeedModel { model_turns })?,
                                events: vec![context_checkpoint_event(
                                    plan.covered_through_sequence(),
                                    summary,
                                )?],
                            })
                        }
                        CompactionContinuation::Explicit => {
                            let context = explicit_context.as_ref().ok_or_else(|| {
                                LoopError::new("explicit compaction context is missing")
                            })?;
                            let estimated_input_tokens = self
                                .context
                                .estimate_after_explicit_compaction(context, &plan, &summary)
                                .map_err(|error| {
                                    LoopError::new(format!(
                                        "explicit context compaction estimate failed: {error}"
                                    ))
                                })?;
                            Ok(LoopDecision::Complete {
                                checkpoint: checkpoint(LoopPhase::Terminal)?,
                                events: vec![
                                    context_checkpoint_event(
                                        plan.covered_through_sequence(),
                                        summary,
                                    )?,
                                    compaction_result_event(estimated_input_tokens)?,
                                ],
                            })
                        }
                    },
                    Err(error) => self.retry_or_fail_compaction(
                        continuation,
                        plan,
                        max_attempts,
                        attempt,
                        error.to_string(),
                    ),
                }
            }
            ModelEffectOutput::ContextWindowExceeded { message } => Self::compaction_failure(
                format!("compaction request exceeded provider context: {message}"),
            ),
        }
    }

    pub(super) fn compact_after_provider_overflow(
        &self,
        model_turns: u32,
        operation_id: renoa_kernel::OperationId,
        events: &[renoa_kernel::SemanticEvent],
        provider_message: &str,
    ) -> Result<LoopDecision, LoopError> {
        if model_turns >= self.config.max_model_turns.get() {
            return Self::compaction_failure(format!(
                "provider rejected model context after the configured model-turn limit: {provider_message}"
            ));
        }
        match self.prepare_context(operation_id, events, true)? {
            ContextPreparation::Compact { plan, max_attempts } => {
                self.invoke_compaction(model_turns, plan, max_attempts)
            }
            ContextPreparation::CapacityExceeded {
                estimated_input_tokens,
                dispatch_limit_tokens,
            } => Self::context_capacity_failure(
                estimated_input_tokens,
                dispatch_limit_tokens,
                Some(provider_message),
            ),
            ContextPreparation::Model { .. } => Self::compaction_failure(format!(
                "provider rejected model context, but the configured context strategy produced no compaction plan: {provider_message}"
            )),
        }
    }

    pub(super) fn context_capacity_failure(
        estimated_input_tokens: u64,
        dispatch_limit_tokens: u64,
        provider_message: Option<&str>,
    ) -> Result<LoopDecision, LoopError> {
        let capacity = format!(
            "context cannot be reduced below the provider limit: estimated {estimated_input_tokens} input tokens, dispatch limit {dispatch_limit_tokens}"
        );
        let reason = match provider_message {
            Some(message) => {
                format!("provider rejected model context: {message}; {capacity}")
            }
            None => capacity,
        };
        Self::compaction_failure(reason)
    }

    fn retry_or_fail_compaction(
        &self,
        continuation: CompactionContinuation,
        plan: CompactionPlan,
        max_attempts: NonZeroU32,
        attempt: NonZeroU32,
        reason: String,
    ) -> Result<LoopDecision, LoopError> {
        if attempt >= max_attempts {
            return Self::compaction_failure(reason);
        }
        let next = attempt
            .get()
            .checked_add(1)
            .and_then(NonZeroU32::new)
            .ok_or_else(|| LoopError::new("compaction attempt counter overflowed"))?;
        self.invoke_compaction_attempt(continuation, plan, max_attempts, next)
    }

    fn compaction_failure(reason: String) -> Result<LoopDecision, LoopError> {
        Ok(LoopDecision::Fail {
            checkpoint: checkpoint(LoopPhase::Terminal)?,
            events: Vec::new(),
            reason,
        })
    }
}

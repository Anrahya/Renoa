use std::{num::NonZeroU32, sync::Arc};

use renoa_agent::{Message, ModelRequest, ModelResponse, ToolSpec};

use super::{CompactionLimits, CompactionPlanner, ContextSizer, checkpoint_message, validation};
use crate::context::{
    CompactionValidationError, ContextInput, ContextPreparation, ContextStrategy,
    ContextStrategyError,
};

/// Renoa's researched portable-summary context strategy.
///
/// This is an ordinary replaceable loop component. It performs no external
/// work: it sizes exact candidate requests, projects an activated checkpoint,
/// and returns a summary plan for the loop to execute through the kernel.
pub struct CompactingContextStrategy {
    planner: CompactionPlanner,
    max_attempts: NonZeroU32,
    sizer: Arc<dyn ContextSizer>,
}

impl CompactingContextStrategy {
    /// Binds validated context limits, a bounded summary-attempt count, and a
    /// deterministic model-aware request sizer.
    #[must_use]
    pub fn new(
        limits: CompactionLimits,
        max_attempts: NonZeroU32,
        sizer: Arc<dyn ContextSizer>,
    ) -> Self {
        Self {
            planner: CompactionPlanner::new(limits),
            max_attempts,
            sizer,
        }
    }

    fn projected_messages(input: &ContextInput) -> Result<Vec<Message>, ContextStrategyError> {
        let Some(checkpoint) = input.active_checkpoint() else {
            return Ok(input.messages().to_vec());
        };
        let anchor = input
            .entries()
            .find(|entry| entry.operation_id() == input.active_operation_id())
            .ok_or_else(|| {
                ContextStrategyError::new("active operation has no durable user message")
            })?;
        if !matches!(anchor.message(), Message::User { .. }) {
            return Err(ContextStrategyError::new(
                "active operation does not start with a user message",
            ));
        }
        let mut messages = vec![checkpoint_message(checkpoint.summary())];
        if anchor.sequence() <= checkpoint.covered_through_sequence() {
            messages.push(anchor.message().clone());
        }
        messages.extend(
            input
                .entries()
                .filter(|entry| entry.sequence() > checkpoint.covered_through_sequence())
                .map(|entry| entry.message().clone()),
        );
        Ok(messages)
    }
}

impl ContextStrategy for CompactingContextStrategy {
    fn project(&self, input: ContextInput) -> Result<Vec<Message>, ContextStrategyError> {
        Self::projected_messages(&input)
    }

    fn prepare(&self, input: ContextInput) -> Result<ContextPreparation, ContextStrategyError> {
        let messages = Self::projected_messages(&input)?;
        let candidate = ModelRequest {
            system_prompt: input.system_prompt().to_owned(),
            messages,
            tools: input.tools().to_vec(),
        };
        let estimated_input_tokens = self.sizer.estimate_input_tokens(&candidate);
        let dispatch_limit_tokens = self.planner.limits.dispatch_limit_tokens().get();
        if !input.compaction_required() && estimated_input_tokens <= dispatch_limit_tokens {
            return Ok(ContextPreparation::Model {
                messages: candidate.messages,
            });
        }
        let plan = self
            .planner
            .plan(
                &input,
                input.active_checkpoint(),
                input.system_prompt(),
                input.tools(),
                self.sizer.as_ref(),
            )
            .map_err(|error| ContextStrategyError::new(error.to_string()))?;
        Ok(match plan {
            Some(plan) => ContextPreparation::Compact {
                plan,
                max_attempts: self.max_attempts,
            },
            None => ContextPreparation::CapacityExceeded {
                estimated_input_tokens,
                dispatch_limit_tokens,
            },
        })
    }

    fn validate_compaction(
        &self,
        _plan: &super::CompactionPlan,
        response: &ModelResponse,
        system_prompt: &str,
        tools: &[ToolSpec],
    ) -> Result<String, CompactionValidationError> {
        validation::summary(
            response,
            system_prompt,
            tools,
            self.planner.limits.max_summary_tokens(),
            self.sizer.as_ref(),
        )
    }
}

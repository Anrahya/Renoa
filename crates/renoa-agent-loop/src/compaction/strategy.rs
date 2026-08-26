use std::{num::NonZeroU32, sync::Arc};

use renoa_agent::{Message, ModelRequest, ModelResponse, ToolSpec};

use super::{CompactionLimits, CompactionPlanner, ContextSizer, checkpoint_message, validation};
use crate::context::{
    CompactionValidationError, ContextInput, ContextPreparation, ContextProjector, ContextStrategy,
    ContextStrategyError, ExplicitCompactionPreparation,
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
    projector: Arc<dyn ContextProjector>,
}

struct UnchangedMessages;

impl ContextProjector for UnchangedMessages {
    fn project(&self, messages: Vec<Message>) -> Result<Vec<Message>, ContextStrategyError> {
        Ok(messages)
    }
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
        Self::with_projector(limits, max_attempts, sizer, Arc::new(UnchangedMessages))
    }

    /// Binds the compactor to a replaceable deterministic message projector.
    ///
    /// The projector runs before every normal and retained-tail size estimate,
    /// making the estimate describe the exact message set that would be sent.
    #[must_use]
    pub fn with_projector(
        limits: CompactionLimits,
        max_attempts: NonZeroU32,
        sizer: Arc<dyn ContextSizer>,
        projector: Arc<dyn ContextProjector>,
    ) -> Self {
        Self {
            planner: CompactionPlanner::new(limits),
            max_attempts,
            sizer,
            projector,
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

    fn projected_idle_messages(
        &self,
        input: &ContextInput,
    ) -> Result<Vec<Message>, ContextStrategyError> {
        let messages = match input.active_checkpoint() {
            None => input.messages().to_vec(),
            Some(checkpoint) => {
                let mut messages = vec![checkpoint_message(checkpoint.summary())];
                messages.extend(
                    input
                        .entries()
                        .filter(|entry| entry.sequence() > checkpoint.covered_through_sequence())
                        .map(|entry| entry.message().clone()),
                );
                messages
            }
        };
        self.projector.project(messages)
    }

    fn estimate_request(&self, input: &ContextInput, messages: Vec<Message>) -> u64 {
        self.sizer.estimate_input_tokens(&ModelRequest {
            system_prompt: input.system_prompt().to_owned(),
            messages,
            tools: input.tools().to_vec(),
        })
    }

    fn checkpoint_is_current(input: &ContextInput) -> bool {
        let latest = input.entries().next_back().map(|entry| entry.sequence());
        match (latest, input.active_checkpoint()) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(latest), Some(checkpoint)) => checkpoint.covered_through_sequence() >= latest,
        }
    }
}

impl ContextStrategy for CompactingContextStrategy {
    fn project(&self, input: ContextInput) -> Result<Vec<Message>, ContextStrategyError> {
        self.projector.project(Self::projected_messages(&input)?)
    }

    fn prepare(&self, input: ContextInput) -> Result<ContextPreparation, ContextStrategyError> {
        let messages = self.projector.project(Self::projected_messages(&input)?)?;
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
            .plan_projected(
                &input,
                input.active_checkpoint(),
                input.system_prompt(),
                input.tools(),
                self.sizer.as_ref(),
                self.projector.as_ref(),
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

    fn prepare_explicit_compaction(
        &self,
        input: &ContextInput,
    ) -> Result<ExplicitCompactionPreparation, ContextStrategyError> {
        let current = self.projected_idle_messages(input)?;
        let estimated_input_tokens = self.estimate_request(input, current);
        if Self::checkpoint_is_current(input) {
            return Ok(ExplicitCompactionPreparation::UpToDate {
                estimated_input_tokens,
            });
        }
        let plan = self
            .planner
            .plan_explicit(input, input.active_checkpoint(), self.sizer.as_ref())
            .map_err(|error| ContextStrategyError::new(error.to_string()))?;
        Ok(match plan {
            Some(plan) => ExplicitCompactionPreparation::Compact {
                plan,
                max_attempts: self.max_attempts,
            },
            None => ExplicitCompactionPreparation::CapacityExceeded {
                estimated_input_tokens,
                dispatch_limit_tokens: self.planner.limits.dispatch_limit_tokens().get(),
            },
        })
    }

    fn estimate_after_explicit_compaction(
        &self,
        input: &ContextInput,
        plan: &super::CompactionPlan,
        summary: &str,
    ) -> Result<u64, ContextStrategyError> {
        let mut messages = vec![checkpoint_message(summary)];
        messages.extend(
            input
                .entries()
                .filter(|entry| entry.sequence() > plan.covered_through_sequence())
                .map(|entry| entry.message().clone()),
        );
        let messages = self.projector.project(messages)?;
        Ok(self.estimate_request(input, messages))
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

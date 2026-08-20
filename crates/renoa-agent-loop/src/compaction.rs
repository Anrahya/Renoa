use std::num::NonZeroU64;

use renoa_agent::{Message, ModelRequest, ToolSpec};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ContextInput;

mod format;
mod planning;
mod strategy;
#[cfg(test)]
mod tests;
mod validation;

pub use strategy::CompactingContextStrategy;

const CHECKPOINT_PREFIX: &str = "[CONTEXT CHECKPOINT]\n";
const CHECKPOINT_SUFFIX: &str = "\n[END CONTEXT CHECKPOINT]";

/// Validated token limits used by pure compaction planning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionLimits {
    dispatch_limit: NonZeroU64,
    tail_budget: NonZeroU64,
    max_summary: NonZeroU64,
}

impl CompactionLimits {
    /// Creates limits with provider headroom and a lower post-compaction target.
    ///
    /// # Errors
    ///
    /// Rejects a reserve that consumes the context window, a target that
    /// reaches the dispatch limit, or a summary budget that consumes the
    /// target.
    pub fn new(
        context_window_tokens: NonZeroU64,
        reserved_tokens: u64,
        target_input_tokens: NonZeroU64,
        max_summary_tokens: NonZeroU64,
    ) -> Result<Self, CompactionLimitsError> {
        let dispatch_limit_tokens = context_window_tokens
            .get()
            .checked_sub(reserved_tokens)
            .and_then(NonZeroU64::new)
            .ok_or(CompactionLimitsError::ReservedTokensExhaustWindow {
                context_window_tokens: context_window_tokens.get(),
                reserved_tokens,
            })?;
        if target_input_tokens >= dispatch_limit_tokens {
            return Err(CompactionLimitsError::TargetNotBelowDispatchLimit {
                target_input_tokens: target_input_tokens.get(),
                dispatch_limit_tokens: dispatch_limit_tokens.get(),
            });
        }
        let tail_budget_tokens = target_input_tokens
            .get()
            .checked_sub(max_summary_tokens.get())
            .and_then(NonZeroU64::new)
            .ok_or(CompactionLimitsError::SummaryNotBelowTarget {
                max_summary_tokens: max_summary_tokens.get(),
                target_input_tokens: target_input_tokens.get(),
            })?;
        Ok(Self {
            dispatch_limit: dispatch_limit_tokens,
            tail_budget: tail_budget_tokens,
            max_summary: max_summary_tokens,
        })
    }

    /// Returns the largest request that may be sent to the provider.
    #[must_use]
    pub const fn dispatch_limit_tokens(self) -> NonZeroU64 {
        self.dispatch_limit
    }

    const fn tail_budget_tokens(self) -> NonZeroU64 {
        self.tail_budget
    }

    const fn max_summary_tokens(self) -> NonZeroU64 {
        self.max_summary
    }
}

/// A previously activated summary and the last durable message it covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionCheckpoint<'a> {
    covered_through_sequence: u64,
    summary: &'a str,
}

impl<'a> CompactionCheckpoint<'a> {
    /// Identifies one activated summary by its claimed durable boundary.
    ///
    /// The planner validates the summary and resolves the boundary against its
    /// [`ContextInput`], because neither invariant can be established here.
    #[must_use]
    pub const fn new(covered_through_sequence: u64, summary: &'a str) -> Self {
        Self {
            covered_through_sequence,
            summary,
        }
    }

    /// Returns the last durable message represented by this checkpoint.
    #[must_use]
    pub const fn covered_through_sequence(self) -> u64 {
        self.covered_through_sequence
    }

    /// Returns the activated portable summary.
    #[must_use]
    pub const fn summary(self) -> &'a str {
        self.summary
    }
}

/// A pure, deterministic summary request and the durable prefix it covers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionPlan {
    summary_request: ModelRequest,
    covered_through_sequence: u64,
}

impl CompactionPlan {
    /// Returns the provider-neutral request that must be persisted before use.
    #[must_use]
    pub const fn summary_request(&self) -> &ModelRequest {
        &self.summary_request
    }

    /// Returns the last durable message included in the summary request.
    #[must_use]
    pub const fn covered_through_sequence(&self) -> u64 {
        self.covered_through_sequence
    }
}

/// The researched bounded planner used by Renoa's durable-summary strategy.
#[derive(Debug, Clone, Copy)]
pub struct CompactionPlanner {
    limits: CompactionLimits,
}

impl CompactionPlanner {
    /// Creates a planner with one validated, revision-frozen limit set.
    #[must_use]
    pub const fn new(limits: CompactionLimits) -> Self {
        Self { limits }
    }

    /// Selects the least-destructive safe prefix whose retained tail fits the
    /// target. If none fits, it returns the largest prefix whose summary input
    /// can still be dispatched, allowing the caller to make bounded progress.
    ///
    /// Call this only after the current model request has exceeded the dispatch
    /// limit or the provider has rejected it for context capacity. `Ok(None)`
    /// means no safe prefix can produce a dispatchable summary request.
    ///
    /// # Errors
    ///
    /// Rejects malformed tool groups, an invalid active user anchor, invalid
    /// checkpoint data, or a summary-request encoding failure.
    pub fn plan(
        &self,
        context: &ContextInput,
        checkpoint: Option<CompactionCheckpoint<'_>>,
        system_prompt: &str,
        tools: &[ToolSpec],
        sizer: &dyn ContextSizer,
    ) -> Result<Option<CompactionPlan>, CompactionPlanningError> {
        planning::select_plan(
            context,
            checkpoint,
            system_prompt,
            tools,
            self.limits,
            sizer,
        )
    }
}

pub(crate) fn validate_plan(
    context: &ContextInput,
    plan: &CompactionPlan,
) -> Result<(), CompactionPlanningError> {
    if plan.summary_request.messages.is_empty() {
        return Err(CompactionPlanningError::InvalidPlan(
            "summary request has no model input".to_owned(),
        ));
    }
    if !plan.summary_request.tools.is_empty() {
        return Err(CompactionPlanningError::InvalidPlan(
            "summary request advertises tools".to_owned(),
        ));
    }
    if context.active_checkpoint().is_some_and(|checkpoint| {
        checkpoint.covered_through_sequence() >= plan.covered_through_sequence
    }) {
        return Err(CompactionPlanningError::InvalidPlan(
            "covered boundary does not advance the active checkpoint".to_owned(),
        ));
    }
    planning::validate_boundary(context, plan.covered_through_sequence)
}

/// Deterministic model-aware sizing for one provider-neutral request.
///
/// Estimates must not decrease when a request is extended without removing
/// existing content. The planner relies on that monotonicity for logarithmic
/// cut selection. Implementations should overestimate when exact provider
/// tokenization is unavailable.
pub trait ContextSizer: Send + Sync {
    /// Estimates provider input tokens for the exact request supplied.
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64;
}

fn checkpoint_message(summary: &str) -> Message {
    Message::user_text(format!("{CHECKPOINT_PREFIX}{summary}{CHECKPOINT_SUFFIX}"))
}

/// Invalid ordering among compaction token limits.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompactionLimitsError {
    #[error(
        "reserved tokens ({reserved_tokens}) consume the context window ({context_window_tokens})"
    )]
    ReservedTokensExhaustWindow {
        context_window_tokens: u64,
        reserved_tokens: u64,
    },
    #[error(
        "post-compaction target ({target_input_tokens}) must be below the dispatch limit ({dispatch_limit_tokens})"
    )]
    TargetNotBelowDispatchLimit {
        target_input_tokens: u64,
        dispatch_limit_tokens: u64,
    },
    #[error(
        "checkpoint summary limit ({max_summary_tokens}) must be below the post-compaction target ({target_input_tokens})"
    )]
    SummaryNotBelowTarget {
        max_summary_tokens: u64,
        target_input_tokens: u64,
    },
}

/// A pure compaction plan could not be derived safely.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CompactionPlanningError {
    #[error("context history is invalid: {0}")]
    InvalidHistory(String),
    #[error("active operation has no durable user message")]
    MissingActiveUser,
    #[error("active operation does not start with a user message")]
    InvalidActiveUser,
    #[error("active context checkpoint is invalid: {0}")]
    InvalidCheckpoint(String),
    #[error("compaction plan is invalid: {0}")]
    InvalidPlan(String),
    #[error("compaction request encoding failed: {0}")]
    RequestEncoding(#[source] serde_json::Error),
}

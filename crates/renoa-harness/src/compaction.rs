use std::num::{NonZeroU32, NonZeroU64};
use std::sync::Arc;

use renoa_agent::ModelRequest;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    OperationId, OperationOutcome, SessionId,
    checkpoint::{ActiveCheckpoint, ContextEntry},
    state::{OperationProgress, StoredState},
};

/// Frozen limits for deciding when and how far to compact model context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionPolicy {
    context_window_tokens: NonZeroU64,
    reserved_tokens: u64,
    target_input_tokens: NonZeroU64,
    max_summary_tokens: NonZeroU64,
    max_attempts: NonZeroU32,
}

impl CompactionPolicy {
    /// Creates a policy with explicit dispatch headroom and a lower target.
    ///
    /// # Errors
    ///
    /// Returns an error when reserved tokens consume the context window, the
    /// target leaves no dispatch headroom, or the summary consumes the target.
    pub fn new(
        context_window_tokens: NonZeroU64,
        reserved_tokens: u64,
        target_input_tokens: NonZeroU64,
        max_summary_tokens: NonZeroU64,
        max_attempts: NonZeroU32,
    ) -> Result<Self, CompactionPolicyError> {
        let dispatch_limit = context_window_tokens
            .get()
            .checked_sub(reserved_tokens)
            .filter(|limit| *limit > 0)
            .ok_or(CompactionPolicyError::ReservedTokensExhaustWindow {
                context_window_tokens: context_window_tokens.get(),
                reserved_tokens,
            })?;
        if target_input_tokens.get() >= dispatch_limit {
            return Err(CompactionPolicyError::TargetNotBelowDispatchLimit {
                target_input_tokens: target_input_tokens.get(),
                dispatch_limit_tokens: dispatch_limit,
            });
        }
        if max_summary_tokens.get() >= target_input_tokens.get() {
            return Err(CompactionPolicyError::SummaryNotBelowTarget {
                max_summary_tokens: max_summary_tokens.get(),
                target_input_tokens: target_input_tokens.get(),
            });
        }
        Ok(Self {
            context_window_tokens,
            reserved_tokens,
            target_input_tokens,
            max_summary_tokens,
            max_attempts,
        })
    }

    pub(crate) const fn frozen(&self) -> FrozenCompaction {
        FrozenCompaction {
            context_window_tokens: self.context_window_tokens.get(),
            reserved_tokens: self.reserved_tokens,
            target_input_tokens: self.target_input_tokens.get(),
            max_summary_tokens: self.max_summary_tokens.get(),
            max_attempts: self.max_attempts.get(),
        }
    }
}

/// Deterministic model-aware sizing for one provider-neutral request.
///
/// Estimates must not decrease when a request is extended without removing
/// existing content. Compaction planning uses that monotonicity to find a safe
/// prefix without rebuilding every possible prefix. Implementations should
/// overestimate when exact provider tokenization is unavailable.
pub trait ContextSizer: Send + Sync {
    fn estimate_input_tokens(&self, request: &ModelRequest) -> u64;
}

pub(crate) struct CompactionBinding {
    pub(crate) policy: CompactionPolicy,
    pub(crate) sizer: Arc<dyn ContextSizer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FrozenCompaction {
    pub(crate) context_window_tokens: u64,
    pub(crate) reserved_tokens: u64,
    pub(crate) target_input_tokens: u64,
    pub(crate) max_summary_tokens: u64,
    pub(crate) max_attempts: u32,
}

impl FrozenCompaction {
    pub(crate) fn dispatch_limit(self) -> Result<u64, crate::HarnessError> {
        self.context_window_tokens
            .checked_sub(self.reserved_tokens)
            .filter(|limit| *limit > 0)
            .ok_or_else(|| {
                crate::HarnessError::Corrupt(
                    "frozen compaction reserve consumes its context window".to_owned(),
                )
            })
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompactionPolicyError {
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

pub(crate) struct CompactionSource {
    pub(crate) progress: OperationProgress,
    pub(crate) checkpoint: Option<ActiveCheckpoint>,
    pub(crate) entries: Vec<ContextEntry>,
}

pub(crate) struct CompactionPlan {
    pub(crate) request: ModelRequest,
    pub(crate) checkpoint_id: Uuid,
    pub(crate) previous_checkpoint_id: Option<Uuid>,
    pub(crate) covered_through_sequence: u64,
}

pub(crate) struct CompactionIntent {
    pub(crate) session_id: SessionId,
    pub(crate) operation_id: OperationId,
    pub(crate) effect_id: Uuid,
    pub(crate) settlement_token: Uuid,
    pub(crate) output_id: Uuid,
    pub(crate) progress: OperationProgress,
    pub(crate) plan: CompactionPlan,
}

pub(crate) enum CompactionStart {
    Invoke(Box<CompactionIntent>),
    Finished(OperationOutcome),
}

pub(crate) enum CompactionRecovery {
    Retry(Box<CompactionIntent>),
    Finished(OperationOutcome),
}

pub(crate) enum CompactionAttempt {
    Retry(Box<CompactionIntent>),
    Continue(StoredState),
    Finished(OperationOutcome),
    Stale,
}

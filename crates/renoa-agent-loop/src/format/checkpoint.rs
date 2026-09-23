use std::num::NonZeroU32;

use renoa_kernel::{Checkpoint, LoopError};
use serde::{Deserialize, Serialize};

use crate::{
    CompactionPlan,
    code_mode::{CodeCallBatch, CodeRun, CodeStepRequest},
    configuration::CHECKPOINT_SCHEMA_VERSION,
    pending_tools::PendingToolCalls,
};

/// The exact decision state needed to resume one unfinished operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum LoopPhase {
    NeedModel {
        model_turns: u32,
    },
    AwaitingModel {
        model_turns: u32,
    },
    AwaitingCompaction {
        model_turns: u32,
        plan: CompactionPlan,
        max_attempts: NonZeroU32,
        attempt: NonZeroU32,
    },
    AwaitingExplicitCompaction {
        plan: CompactionPlan,
        max_attempts: NonZeroU32,
        attempt: NonZeroU32,
    },
    NeedTool {
        model_turns: u32,
        pending: PendingToolCalls,
    },
    AwaitingTool {
        model_turns: u32,
        pending: PendingToolCalls,
    },
    AwaitingCodeStep {
        model_turns: u32,
        pending: PendingToolCalls,
        run: CodeRun,
        request: CodeStepRequest,
    },
    AwaitingCodeCalls {
        model_turns: u32,
        pending: PendingToolCalls,
        run: CodeRun,
        snapshot: String,
        calls: CodeCallBatch,
    },
    Terminal,
}

pub(crate) fn checkpoint(phase: LoopPhase) -> Result<Checkpoint, LoopError> {
    serde_json::to_value(phase)
        .map(|state| Checkpoint::new(CHECKPOINT_SCHEMA_VERSION, state))
        .map_err(|error| LoopError::new(format!("agent checkpoint encoding failed: {error}")))
}

pub(crate) fn decode_checkpoint(checkpoint: &Checkpoint) -> Result<LoopPhase, LoopError> {
    let phase = serde_json::from_value(checkpoint.state().clone())
        .map_err(|error| LoopError::new(format!("agent checkpoint is invalid: {error}")))?;
    let attempts = match &phase {
        LoopPhase::AwaitingCompaction {
            max_attempts,
            attempt,
            ..
        }
        | LoopPhase::AwaitingExplicitCompaction {
            max_attempts,
            attempt,
            ..
        } => Some((attempt, max_attempts)),
        _ => None,
    };
    if attempts.is_some_and(|(attempt, max_attempts)| attempt > max_attempts) {
        return Err(LoopError::new(
            "agent checkpoint compaction attempt exceeds its maximum",
        ));
    }
    match &phase {
        LoopPhase::NeedTool { pending, .. } | LoopPhase::AwaitingTool { pending, .. } => {
            pending.validate()?;
        }
        LoopPhase::AwaitingCodeStep {
            pending,
            run,
            request,
            ..
        } => {
            pending.validate()?;
            run.validate_step(request)?;
        }
        LoopPhase::AwaitingCodeCalls {
            pending,
            run,
            snapshot,
            calls,
            ..
        } => {
            pending.validate()?;
            run.validate_call_wave(snapshot, calls)?;
        }
        _ => {}
    }
    Ok(phase)
}

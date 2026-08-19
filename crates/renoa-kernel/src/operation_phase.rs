use crate::{KernelError, OperationStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperationPhase {
    Queued,
    NeedDecision,
    EffectIntent,
    EffectDispatched,
    OutcomeUnknown,
    Waiting,
    Completed,
    Failed,
    Cancelled,
}

impl OperationPhase {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::NeedDecision => "need_decision",
            Self::EffectIntent => "effect_intent",
            Self::EffectDispatched => "effect_dispatched",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::Waiting => "waiting",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn from_database(value: &str) -> Result<Self, KernelError> {
        match value {
            "queued" => Ok(Self::Queued),
            "need_decision" => Ok(Self::NeedDecision),
            "effect_intent" => Ok(Self::EffectIntent),
            "effect_dispatched" => Ok(Self::EffectDispatched),
            "outcome_unknown" => Ok(Self::OutcomeUnknown),
            "waiting" => Ok(Self::Waiting),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(KernelError::Corrupt(format!(
                "unknown operation phase `{value}`"
            ))),
        }
    }

    pub(crate) const fn status(self) -> OperationStatus {
        match self {
            Self::Queued => OperationStatus::Queued,
            Self::NeedDecision | Self::EffectIntent | Self::EffectDispatched => {
                OperationStatus::Running
            }
            Self::OutcomeUnknown => OperationStatus::OutcomeUnknown,
            Self::Waiting => OperationStatus::Waiting,
            Self::Completed => OperationStatus::Completed,
            Self::Failed => OperationStatus::Failed,
            Self::Cancelled => OperationStatus::Cancelled,
        }
    }

    pub(crate) const fn is_cancellable(self) -> bool {
        matches!(
            self,
            Self::NeedDecision | Self::EffectIntent | Self::EffectDispatched | Self::OutcomeUnknown
        )
    }

    pub(crate) fn active_effect_status(self) -> Result<&'static str, KernelError> {
        match self {
            Self::EffectIntent => Ok("intent_committed"),
            Self::EffectDispatched => Ok("dispatch_started"),
            _ => Err(KernelError::Corrupt(format!(
                "cannot prepare effect from phase `{}`",
                self.as_str()
            ))),
        }
    }
}

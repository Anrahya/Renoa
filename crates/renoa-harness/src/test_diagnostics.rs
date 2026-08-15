use crate::{Harness, SessionId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CrashPoint {
    ActivationCommitted,
    ModelIntentCommitted,
    ModelCompletedBeforeSettlement,
    ContextOverflowCommitted,
    CompactionIntentCommitted,
    CompactionCompletedBeforeSettlement,
    CompactionSettlementCommitted,
    ToolPlanCommitted,
    ToolIntentCommitted,
    ToolCompletedBeforeSettlement,
    ToolSettlementCommitted,
    SettlementCommitted,
}

impl Harness {
    pub(crate) fn crash_at(&mut self, point: CrashPoint) {
        self.crash_point = Some(point);
    }
}

pub(crate) struct ModelAttemptDiagnostic {
    pub(crate) status: String,
    pub(crate) usage: Option<renoa_agent::TokenUsage>,
    pub(crate) has_request: bool,
    pub(crate) error: Option<String>,
}

pub(crate) fn inspect_model_attempts(
    harness: &Harness,
    session_id: SessionId,
) -> Vec<ModelAttemptDiagnostic> {
    harness
        .store
        .inspect_model_attempts(session_id)
        .expect("inspect model attempts")
}

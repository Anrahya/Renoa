use std::sync::Arc;

use renoa_agent::{
    AssistantContent, ModelRequest, ModelResponse, SamplingError, StopReason, Tool, ToolCall,
    invoke_tool, sample_model,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    HarnessError, OperationId, OperationOutcome, RunNext, RuntimeProfile, SessionId,
    SessionRunLease,
    state::{FrozenTool, OperationProgress, StoredOperationState, StoredState, ToolBatch},
    store::Store,
};

pub(crate) struct ActiveOperation {
    pub(crate) operation_id: OperationId,
    pub(crate) state: StoredState,
    #[cfg(test)]
    pub(crate) newly_activated: bool,
}

pub(crate) struct ModelIntent {
    pub(crate) session_id: SessionId,
    pub(crate) operation_id: OperationId,
    pub(crate) effect_id: Uuid,
    pub(crate) settlement_token: Uuid,
    pub(crate) assistant_entry_id: Uuid,
    pub(crate) output_id: Uuid,
    pub(crate) progress: OperationProgress,
    pub(crate) request: ModelRequest,
}

pub(crate) enum ModelStart {
    Invoke(ModelIntent),
    Finished(OperationOutcome),
}

pub(crate) struct PlannedTool {
    pub(crate) session_id: SessionId,
    pub(crate) operation_id: OperationId,
    pub(crate) state_json: String,
    pub(crate) progress: OperationProgress,
    pub(crate) batch: ToolBatch,
    pub(crate) result_entry_id: Uuid,
    pub(crate) call: ToolCall,
    pub(crate) frozen_tool: Option<FrozenTool>,
}

pub(crate) struct ToolIntent {
    pub(crate) session_id: SessionId,
    pub(crate) operation_id: OperationId,
    pub(crate) progress: OperationProgress,
    pub(crate) batch: ToolBatch,
    pub(crate) result_entry_id: Uuid,
    pub(crate) call: ToolCall,
    pub(crate) effect_id: Uuid,
    pub(crate) settlement_token: Uuid,
}

pub(crate) enum ToolStart {
    Invoke(Box<ToolIntent>),
    Finished(OperationOutcome),
}

pub(crate) enum PendingRecovery {
    Retry(ModelIntent),
    Finished(OperationOutcome),
}

pub(crate) enum UncertainAttempt {
    Retry(ModelIntent),
    Finished(OperationOutcome),
    Stale,
}

pub(crate) enum Settlement {
    Applied(OperationOutcome),
    Continue(StoredState),
    Stale,
}

pub(crate) enum ToolSettlement {
    Continue(StoredState),
    Finished(OperationOutcome),
    Stale,
}

pub(crate) enum ToolPendingRecovery {
    Retry(Box<ToolIntent>),
    Blocked,
}

enum DriveStep {
    Continue(Box<StoredState>),
    Finished(OperationOutcome),
    Blocked,
}

pub(crate) async fn run_next(
    store: &Store,
    lease: &std::sync::Arc<SessionRunLease>,
    session_id: SessionId,
    profile: &RuntimeProfile,
    #[cfg(test)] crash_point: Option<crate::CrashPoint>,
) -> Result<RunNext, HarnessError> {
    let Some(active) = store.activate(lease, session_id, profile.frozen()).await? else {
        return Ok(RunNext::Idle);
    };
    lease.bind_operation(active.operation_id)?;
    #[cfg(test)]
    if active.newly_activated {
        crash_if(crash_point, crate::CrashPoint::ActivationCommitted);
    }
    drive_active(
        store,
        lease,
        active,
        profile,
        #[cfg(test)]
        crash_point,
    )
    .await
}

async fn drive_active(
    store: &Store,
    lease: &std::sync::Arc<SessionRunLease>,
    active: ActiveOperation,
    profile: &RuntimeProfile,
    #[cfg(test)] crash_point: Option<crate::CrashPoint>,
) -> Result<RunNext, HarnessError> {
    let operation_id = active.operation_id;
    let mut state = active.state;
    loop {
        let step = match state.state().clone() {
            phase @ (StoredOperationState::NeedModel { .. }
            | StoredOperationState::ModelPending { .. }) => {
                drive_model_phase(
                    store,
                    lease,
                    operation_id,
                    phase,
                    profile,
                    #[cfg(test)]
                    crash_point,
                )
                .await?
            }
            phase @ (StoredOperationState::NeedTool { .. }
            | StoredOperationState::ToolPending { .. }) => {
                drive_tool_phase(
                    store,
                    lease,
                    operation_id,
                    phase,
                    profile,
                    #[cfg(test)]
                    crash_point,
                )
                .await?
            }
            StoredOperationState::ToolOutcomeUnknown { .. } => DriveStep::Blocked,
            StoredOperationState::Queued
            | StoredOperationState::Completed
            | StoredOperationState::Failed { .. } => {
                return Err(HarnessError::Corrupt(
                    "session active pointer references a non-runnable operation".to_owned(),
                ));
            }
        };
        match step {
            DriveStep::Continue(next) => state = *next,
            DriveStep::Finished(outcome) => {
                return Ok(RunNext::Finished {
                    operation_id,
                    outcome,
                });
            }
            DriveStep::Blocked => return Ok(RunNext::Blocked { operation_id }),
        }
    }
}

async fn drive_model_phase(
    store: &Store,
    lease: &Arc<SessionRunLease>,
    operation_id: OperationId,
    phase: StoredOperationState,
    profile: &RuntimeProfile,
    #[cfg(test)] crash_point: Option<crate::CrashPoint>,
) -> Result<DriveStep, HarnessError> {
    let intent = match phase {
        StoredOperationState::NeedModel { progress } => {
            require_profile(&progress.runtime.revision, profile)?;
            let projected_messages =
                crate::projection::project_model_context(store, lease, operation_id, profile)
                    .await?;
            match store
                .begin_model_attempt(lease, operation_id, projected_messages)
                .await?
            {
                ModelStart::Invoke(intent) => intent,
                ModelStart::Finished(outcome) => return Ok(DriveStep::Finished(outcome)),
            }
        }
        StoredOperationState::ModelPending { progress, .. } => {
            require_profile(&progress.runtime.revision, profile)?;
            match store.recover_model_attempt(lease, operation_id).await? {
                PendingRecovery::Retry(intent) => intent,
                PendingRecovery::Finished(outcome) => return Ok(DriveStep::Finished(outcome)),
            }
        }
        _ => {
            return Err(HarnessError::Corrupt(
                "model driver received a non-model phase".to_owned(),
            ));
        }
    };
    let settlement = run_model_effect(
        store,
        lease,
        intent,
        profile,
        #[cfg(test)]
        crash_point,
    )
    .await?;
    model_settlement_step(
        settlement,
        #[cfg(test)]
        crash_point,
    )
}

fn model_settlement_step(
    settlement: Settlement,
    #[cfg(test)] crash_point: Option<crate::CrashPoint>,
) -> Result<DriveStep, HarnessError> {
    match settlement {
        Settlement::Continue(next) => {
            #[cfg(test)]
            if matches!(next.state(), StoredOperationState::NeedTool { .. }) {
                crash_if(crash_point, crate::CrashPoint::ToolPlanCommitted);
            }
            Ok(DriveStep::Continue(Box::new(next)))
        }
        Settlement::Applied(outcome) => {
            #[cfg(test)]
            crash_if(crash_point, crate::CrashPoint::SettlementCommitted);
            Ok(DriveStep::Finished(outcome))
        }
        Settlement::Stale => Err(stale_model_result()),
    }
}

async fn drive_tool_phase(
    store: &Store,
    lease: &Arc<SessionRunLease>,
    operation_id: OperationId,
    phase: StoredOperationState,
    profile: &RuntimeProfile,
    #[cfg(test)] crash_point: Option<crate::CrashPoint>,
) -> Result<DriveStep, HarnessError> {
    match phase {
        StoredOperationState::NeedTool { progress, .. } => {
            require_profile(&progress.runtime.revision, profile)?;
            drive_planned_tool(
                store,
                lease,
                operation_id,
                profile,
                #[cfg(test)]
                crash_point,
            )
            .await
        }
        StoredOperationState::ToolPending { progress, .. } => {
            require_profile(&progress.runtime.revision, profile)?;
            match store.recover_tool_attempt(lease, operation_id).await? {
                ToolPendingRecovery::Retry(intent) => {
                    let tool = resolve_intent_tool(profile, &intent)?;
                    execute_tool_intent(
                        store,
                        lease,
                        *intent,
                        tool,
                        #[cfg(test)]
                        crash_point,
                    )
                    .await
                }
                ToolPendingRecovery::Blocked => Ok(DriveStep::Blocked),
            }
        }
        _ => Err(HarnessError::Corrupt(
            "tool driver received a non-tool phase".to_owned(),
        )),
    }
}

async fn drive_planned_tool(
    store: &Store,
    lease: &Arc<SessionRunLease>,
    operation_id: OperationId,
    profile: &RuntimeProfile,
    #[cfg(test)] crash_point: Option<crate::CrashPoint>,
) -> Result<DriveStep, HarnessError> {
    let planned = store.load_planned_tool(lease, operation_id).await?;
    let Some(frozen_tool) = planned.frozen_tool.as_ref() else {
        let result = invoke_tool(None, planned.call.clone(), CancellationToken::new(), None).await;
        let settlement = store
            .settle_unavailable_tool(lease, planned, result)
            .await?;
        return tool_settlement_step(
            settlement,
            #[cfg(test)]
            crash_point,
        );
    };
    let tool = profile.resolve_tool(frozen_tool)?;
    let intent = match store.begin_tool_intent(lease, planned).await? {
        ToolStart::Invoke(intent) => *intent,
        ToolStart::Finished(outcome) => return Ok(DriveStep::Finished(outcome)),
    };
    #[cfg(test)]
    crash_if(crash_point, crate::CrashPoint::ToolIntentCommitted);
    execute_tool_intent(
        store,
        lease,
        intent,
        tool,
        #[cfg(test)]
        crash_point,
    )
    .await
}

async fn execute_tool_intent(
    store: &Store,
    lease: &Arc<SessionRunLease>,
    intent: ToolIntent,
    tool: Arc<dyn Tool>,
    #[cfg(test)] crash_point: Option<crate::CrashPoint>,
) -> Result<DriveStep, HarnessError> {
    let result = invoke_tool(
        Some(tool.as_ref()),
        intent.call.clone(),
        lease.cancellation(),
        None,
    )
    .await;
    #[cfg(test)]
    crash_if(
        crash_point,
        crate::CrashPoint::ToolCompletedBeforeSettlement,
    );
    let settlement = store.settle_tool(lease, intent, result).await?;
    tool_settlement_step(
        settlement,
        #[cfg(test)]
        crash_point,
    )
}

fn resolve_intent_tool(
    profile: &RuntimeProfile,
    intent: &ToolIntent,
) -> Result<Arc<dyn Tool>, HarnessError> {
    let frozen_tool = intent
        .progress
        .runtime
        .tools
        .iter()
        .find(|tool| tool.spec.name == intent.call.name)
        .ok_or_else(|| {
            HarnessError::Corrupt("pending tool is absent from the frozen profile".to_owned())
        })?;
    profile.resolve_tool(frozen_tool)
}

fn tool_settlement_step(
    settlement: ToolSettlement,
    #[cfg(test)] crash_point: Option<crate::CrashPoint>,
) -> Result<DriveStep, HarnessError> {
    match settlement {
        ToolSettlement::Continue(next) => {
            #[cfg(test)]
            crash_if(crash_point, crate::CrashPoint::ToolSettlementCommitted);
            Ok(DriveStep::Continue(Box::new(next)))
        }
        ToolSettlement::Finished(outcome) => {
            #[cfg(test)]
            crash_if(crash_point, crate::CrashPoint::ToolSettlementCommitted);
            Ok(DriveStep::Finished(outcome))
        }
        ToolSettlement::Stale => Err(HarnessError::Corrupt(
            "the sole session driver produced a stale tool result".to_owned(),
        )),
    }
}

async fn run_model_effect(
    store: &Store,
    lease: &std::sync::Arc<SessionRunLease>,
    mut intent: ModelIntent,
    profile: &RuntimeProfile,
    #[cfg(test)] crash_point: Option<crate::CrashPoint>,
) -> Result<Settlement, HarnessError> {
    loop {
        #[cfg(test)]
        crash_if(crash_point, crate::CrashPoint::ModelIntentCommitted);
        let sampled = sample_model(
            profile.model.as_ref(),
            intent.request.clone(),
            lease.cancellation(),
            None,
        )
        .await;
        match sampled {
            Ok(sampled) => {
                #[cfg(test)]
                crash_if(
                    crash_point,
                    crate::CrashPoint::ModelCompletedBeforeSettlement,
                );
                if let Some(message) = model_response_rejection(&intent, &sampled.response) {
                    return store
                        .reject_model_response(lease, intent, sampled.response.usage, message)
                        .await;
                }
                return store.settle_model(lease, intent, sampled.response).await;
            }
            Err(error) => {
                let message = sampling_failure(&error);
                match store
                    .record_model_uncertainty(lease, intent, message)
                    .await?
                {
                    UncertainAttempt::Retry(next) => intent = next,
                    UncertainAttempt::Finished(outcome) => {
                        return Ok(Settlement::Applied(outcome));
                    }
                    UncertainAttempt::Stale => return Err(stale_model_result()),
                }
            }
        }
    }
}

fn stale_model_result() -> HarnessError {
    HarnessError::Corrupt("the sole session driver produced a stale model result".to_owned())
}

#[cfg(test)]
fn crash_if(selected: Option<crate::CrashPoint>, reached: crate::CrashPoint) {
    assert_ne!(selected, Some(reached), "injected crash at {reached:?}");
}

fn require_profile(required: &str, profile: &RuntimeProfile) -> Result<(), HarnessError> {
    if required == profile.revision {
        Ok(())
    } else {
        Err(HarnessError::RuntimeProfileUnavailable {
            required: required.to_owned(),
            provided: profile.revision.clone(),
        })
    }
}

fn sampling_failure(error: &SamplingError) -> String {
    match error {
        SamplingError::Cancelled => "model sampling was cancelled".to_owned(),
        SamplingError::Model(error) => format!("model invocation failed: {error}"),
        SamplingError::IncompleteStream => {
            "model stream ended without a completed response".to_owned()
        }
        _ => "model sampling failed for an unrecognized reason".to_owned(),
    }
}

fn model_response_rejection(intent: &ModelIntent, response: &ModelResponse) -> Option<String> {
    let tool_call_count = response
        .content
        .iter()
        .filter(|content| matches!(content, AssistantContent::ToolCall { .. }))
        .count();
    if tool_call_count > intent.progress.runtime.max_tool_calls_per_step as usize {
        Some(format!(
            "model returned {tool_call_count} tool calls; the per-step limit is {}",
            intent.progress.runtime.max_tool_calls_per_step
        ))
    } else if response.stop_reason == StopReason::ToolUse && tool_call_count == 0 {
        Some("model ended for tool use without returning a tool call".to_owned())
    } else {
        None
    }
}

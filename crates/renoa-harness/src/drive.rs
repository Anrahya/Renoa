use renoa_agent::{
    AssistantContent, ModelRequest, ModelResponse, SamplingError, StopReason, sample_model,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    HarnessError, OperationId, OperationOutcome, RunNext, RuntimeProfile, SessionId,
    SessionRunLease,
    state::{StoredOperationState, StoredState},
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
    pub(crate) runtime_revision: String,
    pub(crate) max_model_attempts: u32,
    pub(crate) attempt_count: u32,
    pub(crate) request: ModelRequest,
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
    Stale,
}

pub(crate) async fn run_next(
    store: &Store,
    lease: &std::sync::Arc<SessionRunLease>,
    session_id: SessionId,
    profile: &RuntimeProfile,
    #[cfg(test)] crash_point: Option<crate::CrashPoint>,
) -> Result<RunNext, HarnessError> {
    let Some(active) = store
        .activate(
            lease,
            session_id,
            &profile.revision,
            &profile.system_prompt,
            profile.max_model_attempts.get(),
        )
        .await?
    else {
        return Ok(RunNext::Idle);
    };
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
    let intent = match active.state.state() {
        StoredOperationState::NeedModel {
            runtime_revision, ..
        } => {
            require_profile(runtime_revision, profile)?;
            store
                .begin_model_attempt(lease, active.operation_id)
                .await?
        }
        StoredOperationState::ModelPending {
            runtime_revision, ..
        } => {
            require_profile(runtime_revision, profile)?;
            match store
                .recover_model_attempt(lease, active.operation_id)
                .await?
            {
                PendingRecovery::Retry(intent) => intent,
                PendingRecovery::Finished(outcome) => {
                    return Ok(RunNext::Finished {
                        operation_id: active.operation_id,
                        outcome,
                    });
                }
            }
        }
        StoredOperationState::Queued
        | StoredOperationState::Completed
        | StoredOperationState::Failed => {
            return Err(HarnessError::Corrupt(
                "session active pointer references a non-runnable operation".to_owned(),
            ));
        }
    };
    let mut intent = intent;
    let outcome = loop {
        #[cfg(test)]
        crash_if(crash_point, crate::CrashPoint::ModelIntentCommitted);

        let sampled = sample_model(
            profile.model.as_ref(),
            intent.request.clone(),
            CancellationToken::new(),
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
                break if let Some(message) = model_only_rejection(&sampled.response) {
                    require_applied(
                        store
                            .reject_model_response(lease, intent, sampled.response.usage, message)
                            .await?,
                    )?
                } else {
                    require_applied(store.settle_model(lease, intent, sampled.response).await?)?
                };
            }
            Err(error) => {
                let message = sampling_failure(&error);
                match store
                    .record_model_uncertainty(lease, intent, message)
                    .await?
                {
                    UncertainAttempt::Retry(next) => intent = next,
                    UncertainAttempt::Finished(outcome) => break outcome,
                    UncertainAttempt::Stale => {
                        return Err(HarnessError::Corrupt(
                            "the sole session driver produced a stale model result".to_owned(),
                        ));
                    }
                }
            }
        }
    };
    #[cfg(test)]
    crash_if(crash_point, crate::CrashPoint::SettlementCommitted);
    Ok(RunNext::Finished {
        operation_id: active.operation_id,
        outcome,
    })
}

fn require_applied(settlement: Settlement) -> Result<crate::OperationOutcome, HarnessError> {
    match settlement {
        Settlement::Applied(outcome) => Ok(outcome),
        Settlement::Stale => Err(HarnessError::Corrupt(
            "the sole session driver produced a stale model result".to_owned(),
        )),
    }
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

fn model_only_rejection(response: &ModelResponse) -> Option<String> {
    let has_tool_call = response
        .content
        .iter()
        .any(|content| matches!(content, AssistantContent::ToolCall { .. }));
    if has_tool_call || response.stop_reason == StopReason::ToolUse {
        Some("model returned a tool call when no tools were advertised".to_owned())
    } else {
        None
    }
}

use std::sync::Arc;

use renoa_agent::{
    AgentEventSink, AssistantContent, ModelErrorKind, ModelResponse, SamplingError, StopReason,
    sample_model, validate_tool_call_ids,
};

use crate::{
    HarnessError, OperationId, RuntimeProfile, SessionRunLease,
    drive::{
        DriveStep, ModelIntent, ModelStart, PendingRecovery, Settlement, UncertainAttempt,
        require_profile,
    },
    state::StoredOperationState,
    store::Store,
};

#[cfg(test)]
use crate::drive::crash_if;

pub(crate) async fn drive_model_phase(
    store: &Store,
    lease: &Arc<SessionRunLease>,
    operation_id: OperationId,
    phase: StoredOperationState,
    profile: &RuntimeProfile,
    sink: Option<&dyn AgentEventSink>,
    #[cfg(test)] crash_point: Option<crate::CrashPoint>,
) -> Result<DriveStep, HarnessError> {
    let intent = match phase {
        StoredOperationState::NeedModel { progress } => {
            require_profile(&progress.runtime.revision, profile)?;
            let projected_request = crate::projection::project_model_request(
                store,
                lease,
                operation_id,
                &progress,
                profile,
            )
            .await?;
            if let Some((estimated_tokens, _)) =
                oversized_context(profile, &progress, projected_request.as_ref())?
            {
                let source = store.load_compaction_source(lease, operation_id).await?;
                return crate::compaction_drive::compact_oversized_context(
                    store,
                    lease,
                    operation_id,
                    profile,
                    source,
                    estimated_tokens,
                    #[cfg(test)]
                    crash_point,
                )
                .await;
            }
            match store
                .begin_model_attempt(lease, operation_id, projected_request)
                .await?
            {
                ModelStart::Invoke(intent) => *intent,
                ModelStart::Finished(outcome) => return Ok(DriveStep::Finished(outcome)),
            }
        }
        StoredOperationState::ModelPending { progress, .. } => {
            require_profile(&progress.runtime.revision, profile)?;
            match store.recover_model_attempt(lease, operation_id).await? {
                PendingRecovery::Retry(intent) => *intent,
                PendingRecovery::Finished(outcome) => return Ok(DriveStep::Finished(outcome)),
            }
        }
        StoredOperationState::CompactionPending { progress, .. } => {
            require_profile(&progress.runtime.revision, profile)?;
            return crate::compaction_drive::recover_compaction(
                store,
                lease,
                operation_id,
                profile,
                #[cfg(test)]
                crash_point,
            )
            .await;
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
        sink,
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

fn oversized_context(
    profile: &RuntimeProfile,
    progress: &crate::state::OperationProgress,
    request: Option<&renoa_agent::ModelRequest>,
) -> Result<Option<(u64, u64)>, HarnessError> {
    let (Some(frozen), Some(request)) = (progress.runtime.compaction, request) else {
        return Ok(None);
    };
    let estimated_tokens = profile
        .resolve_context_sizer(frozen)?
        .estimate_input_tokens(request);
    let dispatch_limit_tokens = frozen.dispatch_limit()?;
    if progress.force_compaction || estimated_tokens > dispatch_limit_tokens {
        return Ok(Some((estimated_tokens, dispatch_limit_tokens)));
    }
    Ok(None)
}

fn model_settlement_step(
    settlement: Settlement,
    #[cfg(test)] crash_point: Option<crate::CrashPoint>,
) -> Result<DriveStep, HarnessError> {
    match settlement {
        Settlement::Continue(next) => {
            #[cfg(test)]
            if matches!(
                next.state(),
                StoredOperationState::NeedModel { progress } if progress.force_compaction
            ) {
                crash_if(crash_point, crate::CrashPoint::ContextOverflowCommitted);
            }
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

async fn run_model_effect(
    store: &Store,
    lease: &Arc<SessionRunLease>,
    mut intent: ModelIntent,
    profile: &RuntimeProfile,
    sink: Option<&dyn AgentEventSink>,
    #[cfg(test)] crash_point: Option<crate::CrashPoint>,
) -> Result<Settlement, HarnessError> {
    loop {
        #[cfg(test)]
        crash_if(crash_point, crate::CrashPoint::ModelIntentCommitted);
        let sampled = sample_model(
            profile.model.as_ref(),
            intent.request.clone(),
            lease.cancellation(),
            sink,
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
                if let SamplingError::Model(error) = &error
                    && error.kind() == ModelErrorKind::ContextWindowExceeded
                {
                    return store
                        .record_context_overflow(lease, intent, error.to_string())
                        .await;
                }
                let message = sampling_failure(&error);
                match store
                    .record_model_uncertainty(lease, intent, message)
                    .await?
                {
                    UncertainAttempt::Retry(next) => intent = *next,
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
    } else if let Err(error) =
        validate_tool_call_ids(response.content.iter().filter_map(|content| match content {
            AssistantContent::ToolCall { call } => Some(call.id.as_str()),
            AssistantContent::Text { .. } | AssistantContent::Reasoning { .. } => None,
        }))
    {
        Some(error.to_string())
    } else if response.stop_reason == StopReason::ToolUse && tool_call_count == 0 {
        Some("model ended for tool use without returning a tool call".to_owned())
    } else {
        None
    }
}

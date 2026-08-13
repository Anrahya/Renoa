use std::sync::Arc;

use renoa_agent::{
    AssistantContent, ModelErrorKind, ModelRequest, ModelResponse, SamplingError, StopReason,
};

use crate::{
    HarnessError, OperationId, RuntimeProfile, SessionRunLease,
    checkpoint::checkpoint_message,
    compaction::{
        CompactionAttempt, CompactionIntent, CompactionRecovery, CompactionSource, CompactionStart,
    },
    drive::DriveStep,
    store::Store,
};

const REQUIRED_HEADINGS: [&str; 7] = [
    "## Goal and user intent",
    "## Hard constraints and preferences",
    "## Completed work",
    "## Current state and blockers",
    "## Decisions and rationale",
    "## Exact working facts",
    "## Next action and unresolved questions",
];

pub(crate) async fn compact_oversized_context(
    store: &Store,
    lease: &Arc<SessionRunLease>,
    operation_id: OperationId,
    profile: &RuntimeProfile,
    source: CompactionSource,
    estimated_tokens: u64,
    #[cfg(test)] crash_point: Option<crate::CrashPoint>,
) -> Result<DriveStep, HarnessError> {
    let frozen = source.progress.runtime.compaction.ok_or_else(|| {
        HarnessError::Corrupt("oversized context has no frozen compaction policy".to_owned())
    })?;
    let sizer = profile.resolve_context_sizer(frozen)?;
    let plan =
        crate::compaction_planning::select_plan(operation_id, &source, frozen, sizer.as_ref())?;
    let Some(plan) = plan else {
        let outcome = store
            .finish_context_capacity_failure(
                lease,
                operation_id,
                estimated_tokens,
                frozen.dispatch_limit()?,
            )
            .await?;
        return Ok(DriveStep::Finished(outcome));
    };
    let intent = match store.begin_compaction(lease, operation_id, plan).await? {
        CompactionStart::Invoke(intent) => *intent,
        CompactionStart::Finished(outcome) => return Ok(DriveStep::Finished(outcome)),
    };
    run_compaction_effect(
        store,
        lease,
        profile,
        intent,
        #[cfg(test)]
        crash_point,
    )
    .await
}

pub(crate) async fn recover_compaction(
    store: &Store,
    lease: &Arc<SessionRunLease>,
    operation_id: OperationId,
    profile: &RuntimeProfile,
    #[cfg(test)] crash_point: Option<crate::CrashPoint>,
) -> Result<DriveStep, HarnessError> {
    let intent = match store.recover_compaction(lease, operation_id).await? {
        CompactionRecovery::Retry(intent) => *intent,
        CompactionRecovery::Finished(outcome) => return Ok(DriveStep::Finished(outcome)),
    };
    run_compaction_effect(
        store,
        lease,
        profile,
        intent,
        #[cfg(test)]
        crash_point,
    )
    .await
}

async fn run_compaction_effect(
    store: &Store,
    lease: &Arc<SessionRunLease>,
    profile: &RuntimeProfile,
    mut intent: CompactionIntent,
    #[cfg(test)] crash_point: Option<crate::CrashPoint>,
) -> Result<DriveStep, HarnessError> {
    loop {
        #[cfg(test)]
        crate::drive::crash_if(crash_point, crate::CrashPoint::CompactionIntentCommitted);
        let sampled = renoa_agent::sample_model(
            profile.model.as_ref(),
            intent.plan.request.clone(),
            lease.cancellation(),
            None,
        )
        .await;
        let result = match sampled {
            Ok(sampled) => {
                #[cfg(test)]
                crate::drive::crash_if(
                    crash_point,
                    crate::CrashPoint::CompactionCompletedBeforeSettlement,
                );
                match validate_summary(profile, &intent, &sampled.response) {
                    Ok(summary) => {
                        store
                            .settle_compaction(lease, intent, summary, sampled.response.usage)
                            .await?
                    }
                    Err(message) => {
                        store
                            .reject_compaction(lease, intent, sampled.response.usage, message)
                            .await?
                    }
                }
            }
            Err(SamplingError::Model(error))
                if error.kind() == ModelErrorKind::ContextWindowExceeded =>
            {
                store
                    .fail_compaction_context_overflow(lease, intent, error.to_string())
                    .await?
            }
            Err(error) => {
                store
                    .record_compaction_uncertainty(lease, intent, sampling_failure(&error))
                    .await?
            }
        };
        match result {
            CompactionAttempt::Retry(next) => intent = *next,
            CompactionAttempt::Continue(state) => {
                #[cfg(test)]
                crate::drive::crash_if(
                    crash_point,
                    crate::CrashPoint::CompactionSettlementCommitted,
                );
                return Ok(DriveStep::Continue(Box::new(state)));
            }
            CompactionAttempt::Finished(outcome) => {
                return Ok(DriveStep::Finished(outcome));
            }
            CompactionAttempt::Stale => {
                return Err(HarnessError::Corrupt(
                    "the sole session driver produced a stale compaction result".to_owned(),
                ));
            }
        }
    }
}

fn validate_summary(
    profile: &RuntimeProfile,
    intent: &CompactionIntent,
    response: &ModelResponse,
) -> Result<String, String> {
    if response.stop_reason != StopReason::Stop {
        return Err("compaction response did not stop normally".to_owned());
    }
    if response
        .content
        .iter()
        .any(|content| matches!(content, AssistantContent::ToolCall { .. }))
    {
        return Err("compaction response attempted to call a tool".to_owned());
    }
    let summary = response
        .content
        .iter()
        .filter_map(|content| match content {
            AssistantContent::Text { text, .. } => Some(text.as_str()),
            AssistantContent::Reasoning { .. } | AssistantContent::ToolCall { .. } => None,
        })
        .collect::<String>();
    validate_sections(&summary)?;
    let frozen = intent
        .progress
        .runtime
        .compaction
        .ok_or_else(|| "pending compaction lost its frozen policy".to_owned())?;
    let request = checkpoint_footprint(&intent.progress.runtime, &summary);
    let estimated = profile
        .resolve_context_sizer(frozen)
        .map_err(|error| error.to_string())?
        .estimate_input_tokens(&request);
    if estimated > frozen.max_summary_tokens {
        return Err(format!(
            "checkpoint alone requires an estimated {estimated} tokens, above its limit {}",
            frozen.max_summary_tokens
        ));
    }
    Ok(summary)
}

fn validate_sections(summary: &str) -> Result<(), String> {
    let summary = summary.trim();
    if summary.is_empty() {
        return Err("compaction response was empty".to_owned());
    }
    let mut lines = summary.lines().peekable();
    for heading in REQUIRED_HEADINGS {
        let actual = lines
            .next()
            .map(str::trim)
            .ok_or_else(|| format!("compaction response is missing `{heading}`"))?;
        if actual != heading {
            return Err(format!(
                "compaction response expected `{heading}`, found `{actual}`"
            ));
        }
        let mut has_content = false;
        while lines
            .peek()
            .is_some_and(|line| !line.trim().starts_with("## "))
        {
            has_content |= !lines.next().expect("peeked line exists").trim().is_empty();
        }
        if !has_content {
            return Err(format!("compaction section `{heading}` is empty"));
        }
    }
    if let Some(extra) = lines.next() {
        return Err(format!(
            "compaction response contains an unexpected heading `{}`",
            extra.trim()
        ));
    }
    Ok(())
}

fn checkpoint_footprint(runtime: &crate::state::FrozenRuntime, summary: &str) -> ModelRequest {
    ModelRequest {
        system_prompt: runtime.system_prompt.clone(),
        messages: vec![checkpoint_message(summary)],
        tools: runtime.tools.iter().map(|tool| tool.spec.clone()).collect(),
    }
}

fn sampling_failure(error: &SamplingError) -> String {
    match error {
        SamplingError::Cancelled => "compaction sampling was cancelled".to_owned(),
        SamplingError::Model(error) => format!("compaction model invocation failed: {error}"),
        SamplingError::IncompleteStream => {
            "compaction model stream ended without a completed response".to_owned()
        }
        _ => "compaction sampling failed for an unrecognized reason".to_owned(),
    }
}

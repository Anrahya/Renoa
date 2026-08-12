use std::sync::Arc;

use renoa_core::{
    CapabilityOutcome, CapabilityRequest, CapabilitySpec, CommandEnvelope, ModelResponse,
    RunEventKind, RunId, StoreError,
};
use tokio_util::sync::CancellationToken;

use super::{Engine, EngineError};

impl Engine {
    pub(super) async fn capability_batch(
        &self,
        run_id: RunId,
        command: &CommandEnvelope,
        response: &ModelResponse,
        capabilities: &[CapabilitySpec],
        cancellation: CancellationToken,
    ) -> Result<Vec<(CapabilityRequest, CapabilityOutcome)>, EngineError> {
        if response.capability_calls.len() > self.config.max_capability_calls_per_response {
            return Err(EngineError::CapabilityBatchTooLarge {
                actual: response.capability_calls.len(),
                limit: self.config.max_capability_calls_per_response,
            });
        }
        let requests = response
            .capability_calls
            .iter()
            .enumerate()
            .map(|(ordinal, call)| {
                let ordinal =
                    u32::try_from(ordinal).map_err(|_| EngineError::CapabilityBatchTooLarge {
                        actual: response.capability_calls.len(),
                        limit: self.config.max_capability_calls_per_response,
                    })?;
                Ok(CapabilityRequest {
                    run_id,
                    target: command.target.clone(),
                    ordinal,
                    call: call.clone(),
                })
            })
            .collect::<Result<Vec<_>, EngineError>>()?;

        self.store
            .append_events(
                run_id,
                requests
                    .iter()
                    .map(|request| RunEventKind::CapabilityRequested {
                        ordinal: request.ordinal,
                        call: request.call.clone(),
                    })
                    .collect(),
            )
            .await?;
        let outcomes = if response.truncated {
            let mut outcomes = Vec::with_capacity(requests.len());
            for request in requests {
                let outcome = CapabilityOutcome::error(
                    "capability call was not executed because the model response was truncated",
                );
                self.record_capability_completion(run_id, &request, &outcome)
                    .await?;
                outcomes.push((request, outcome));
            }
            outcomes
        } else {
            self.execute_capabilities(run_id, requests, capabilities, cancellation)
                .await?
        };
        Ok(outcomes)
    }

    async fn execute_capabilities(
        &self,
        run_id: RunId,
        requests: Vec<CapabilityRequest>,
        capabilities: &[CapabilitySpec],
        cancellation: CancellationToken,
    ) -> Result<Vec<(CapabilityRequest, CapabilityOutcome)>, EngineError> {
        let mut tasks = tokio::task::JoinSet::new();
        let mut outcomes = Vec::with_capacity(requests.len());
        for request in requests {
            if !capabilities
                .iter()
                .any(|capability| capability.name == request.call.name)
            {
                let message = format!("capability `{}` is not granted", request.call.name);
                let outcome = CapabilityOutcome::error(message);
                self.record_capability_completion(run_id, &request, &outcome)
                    .await?;
                outcomes.push((request, outcome));
                continue;
            }
            let host = Arc::clone(&self.capabilities);
            let child_cancellation = cancellation.child_token();
            tasks.spawn(async move {
                let outcome = tokio::select! {
                    () = child_cancellation.cancelled() => {
                        CapabilityOutcome::error("capability execution was cancelled")
                    }
                    outcome = host.execute(request.clone(), child_cancellation.child_token()) => outcome,
                };
                (request, outcome)
            });
            let Some(result) = tasks.join_next().await else {
                return Err(EngineError::CapabilityTask(
                    "capability task disappeared before completion".to_owned(),
                ));
            };
            let (request, outcome) =
                result.map_err(|error| EngineError::CapabilityTask(error.to_string()))?;
            self.record_capability_completion(run_id, &request, &outcome)
                .await?;
            outcomes.push((request, outcome));
        }
        Ok(outcomes)
    }

    async fn record_capability_completion(
        &self,
        run_id: RunId,
        request: &CapabilityRequest,
        outcome: &CapabilityOutcome,
    ) -> Result<(), StoreError> {
        self.store
            .append_events(
                run_id,
                vec![RunEventKind::CapabilityCompleted {
                    ordinal: request.ordinal,
                    call_id: request.call.call_id.clone(),
                    outcome: outcome.clone(),
                }],
            )
            .await
    }
}

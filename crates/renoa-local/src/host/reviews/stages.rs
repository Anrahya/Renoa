//! Stage execution reuses the Host's diagnostic store without changing kernel truth.
use super::{GitHubReviewReport, GitHubReviewSnapshot, findings, reviewer};
use crate::{LocalHost, LocalHostError, LocalSession, LocalTurnOutcome, trace::TraceStore};
use renoa_agent::{AgentEventSink, ContentBlock};
use renoa_kernel::{CommandId, SessionId};
use sha2::{Digest as _, Sha256};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(super) enum ReportStageResult {
    Complete(GitHubReviewReport),
    Incomplete(String),
}

impl LocalHost {
    pub(super) async fn review_report(
        &self,
        session: &LocalSession,
        snapshot: &GitHubReviewSnapshot,
        tools: &reviewer::ReviewTools<'_>,
        validation: bool,
        mut prompt: String,
        cancel: &CancellationToken,
    ) -> Result<ReportStageResult, LocalHostError> {
        let original_id = if validation {
            let mut bytes = *snapshot.request.id.as_bytes();
            bytes[0] ^= 0x80;
            Uuid::from_bytes(bytes)
        } else {
            snapshot.request.id
        };
        let mut invalid = std::collections::BTreeSet::new();
        let mut attempt = 0_u64;
        loop {
            super::active(cancel)?;
            let id = if attempt == 0 {
                original_id
            } else {
                let digest =
                    Sha256::digest(format!("renoa.review.report/v1/{original_id}/{attempt}"));
                Uuid::from_bytes(
                    digest[..16]
                        .try_into()
                        .expect("SHA-256 has sixteen prefix bytes"),
                )
            };
            let outcome = self
                .review_stage(session, snapshot, tools, id, prompt, cancel)
                .await?;
            match outcome {
                LocalTurnOutcome::Completed { output, stop_reason: renoa_agent::StopReason::Stop } => {
                    match findings::parse(&output) {
                        Ok(report) => return Ok(ReportStageResult::Complete(report)),
                        Err(error) => {
                            // Repeating identical invalid output is a stalled
                            // correction, not a reason to rerun the investigation.
                            if !invalid.insert(Sha256::digest(output.as_bytes())) {
                                return Ok(ReportStageResult::Incomplete(format!("Report correction repeated the same invalid output: {error}")));
                            }
                            prompt = serde_json::json!({"task":"Correct the previous review report to the required JSON schema. Preserve supported findings; do not restart the investigation. evidence must be one object {path,start_line,side,quote}, not an array; choose the strongest exact citation. Return only {findings:[...],limitations:[...]}. The previous output and investigation remain in this durable conversation.","schema_error":error.to_string()}).to_string();
                        }
                    }
                }
                LocalTurnOutcome::Failed { reason } => {
                    let stage = if validation { "Validation" } else { "Investigation" };
                    return Ok(ReportStageResult::Incomplete(format!("{stage} failed: {reason}")));
                }
                _ => return Ok(ReportStageResult::Incomplete("Review stage stopped before a complete report; inspect the durable transcript.".to_owned())),
            }
            attempt = attempt.checked_add(1).ok_or_else(|| {
                LocalHostError::InvalidRequest("report correction identity exhausted".to_owned())
            })?;
        }
    }

    async fn review_stage(
        &self,
        session: &LocalSession,
        snapshot: &GitHubReviewSnapshot,
        tools: &reviewer::ReviewTools<'_>,
        id: Uuid,
        prompt: String,
        cancel: &CancellationToken,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        let command = CommandId::from_uuid(id);
        let content = vec![ContentBlock::text(prompt)];
        if let Some(outcome) = session.replay_settled_turn(command, &content)? {
            return Ok(outcome);
        }
        let path = self
            .config
            .database
            .with_file_name("review-sessions")
            .join(snapshot.request.id.to_string())
            .join(crate::trace::TRACE_DATABASE);
        let agent_id = snapshot.request.repository.policy.agent_id;
        let profile = self
            .agent(agent_id)
            .await?
            .ok_or(LocalHostError::AgentNotFound(agent_id))?
            .profile;
        let session_id = SessionId::from_uuid(snapshot.request.id);
        let store = tokio::task::spawn_blocking(move || {
            if path.try_exists()? {
                Ok::<_, LocalHostError>(TraceStore::open(path, session_id, agent_id, &profile)?)
            } else {
                Ok(TraceStore::create(path, session_id, agent_id, &profile)?)
            }
        })
        .await??;
        let trace = store
            .start_run(
                command,
                &content,
                snapshot.provider.as_str(),
                &snapshot.model,
                snapshot.reasoning.as_str(),
            )
            .await?;
        let events: Arc<dyn AgentEventSink> = trace.clone();
        let result = async {
            let runtime = reviewer::runtime(&self.config, snapshot, tools, events).await?;
            Ok(session
                .execute_turn(command, content, &runtime, cancel.child_token())
                .await?)
        }
        .await;
        crate::agent_trace::finish_trace(&trace, &result).await;
        result
    }
}

//! Stage execution reuses the Host's diagnostic store without changing kernel truth.
use super::{GitHubReviewSnapshot, reviewer};
use crate::{LocalHost, LocalHostError, LocalSession, LocalTurnOutcome, trace::TraceStore};
use renoa_agent::{AgentEventSink, ContentBlock};
use renoa_kernel::{CommandId, SessionId};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

impl LocalHost {
    pub(super) async fn review_stage(
        &self,
        session: &LocalSession,
        snapshot: &GitHubReviewSnapshot,
        tools: &reviewer::ReviewTools<'_>,
        validation: bool,
        prompt: String,
        cancel: &CancellationToken,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        let id = if validation {
            let mut bytes = *snapshot.request.id.as_bytes();
            bytes[0] ^= 0x80;
            Uuid::from_bytes(bytes)
        } else {
            snapshot.request.id
        };
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

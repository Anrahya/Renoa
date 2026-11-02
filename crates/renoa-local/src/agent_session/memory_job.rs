//! One `renoa.memory` job as a command of an agent's memory session: Mnem's
//! job prompt, admitted at the time it was frozen, answered with no tools and
//! no person, context or history around it.

use std::sync::Arc;

use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock};
use renoa_agent_loop::{AgentCommand, TurnContext};
use renoa_kernel::{CommandId, Runtime};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{AgentSession, execution::SessionCommand};
use crate::{
    LocalHostError, LocalTurnOutcome, ModelChoice, ReasoningLevel,
    runtime::build_memory_job_runtime, session::PromptAdmission,
};

/// A memory job's frozen input: its prompt and the time it was made.
pub(super) struct MemoryJob {
    pub(super) content: Vec<ContentBlock>,
    pub(super) frozen_at_ms: i64,
}

/// Nobody watches a memory job live; its trace records it.
struct Unwatched;

impl AgentEventSink for Unwatched {
    fn emit(&self, _: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

impl AgentSession {
    /// Runs one memory job under `request_id`, or resumes or replays the one
    /// already admitted under it. The caller freezes `prompt` and
    /// `frozen_at_ms` before the first attempt and passes the same values on
    /// every retry; the kernel refuses a different prompt under the same id.
    /// It is never cancelled: a stopped process leaves it to be resumed.
    ///
    /// # Errors
    ///
    /// Returns request coordination, runtime resolution, admission, or
    /// execution failures; the job may then be unfinished and is retried.
    pub(crate) async fn execute_memory_job(
        &self,
        request_id: Uuid,
        prompt: &str,
        frozen_at_ms: i64,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        self.execute(
            request_id,
            SessionCommand::MemoryJob(MemoryJob {
                content: vec![ContentBlock::text(prompt)],
                frozen_at_ms,
            }),
            Arc::new(Unwatched),
            CancellationToken::new(),
        )
        .await
    }

    /// The runtime every memory job of this session runs with: the session's
    /// model and reasoning, Host-owned instructions, and no tools.
    pub(super) async fn memory_job_runtime(
        &self,
        model: &ModelChoice,
        reasoning: ReasoningLevel,
        events: Arc<dyn AgentEventSink>,
    ) -> Result<Runtime, LocalHostError> {
        Ok(build_memory_job_runtime(
            self.host.bridge.clone(),
            self.host.credential_store.clone(),
            model,
            reasoning,
            renoa_kernel::SessionId::from_uuid(self.id),
            events,
        )
        .await?)
    }

    /// Admits a memory job with no message context, or resumes the command
    /// already admitted under `command_id`, and drives it.
    pub(super) async fn admit_memory_job(
        &self,
        command_id: CommandId,
        job: MemoryJob,
        runtime: &Runtime,
        cancellation: CancellationToken,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        let command = match self.kernel.prompt_admission(command_id, &job.content)? {
            PromptAdmission::Admitted(command) => command,
            PromptAdmission::New { .. } => {
                AgentCommand::observed(job.content, job.frozen_at_ms, TurnContext::default())
                    .map_err(crate::LocalSessionError::from)?
            }
        };
        Ok(self
            .kernel
            .execute_prompt(command_id, command, runtime, cancellation)
            .await?)
    }
}

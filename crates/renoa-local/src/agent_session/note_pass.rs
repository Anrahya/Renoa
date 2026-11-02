//! A `renoa.memory` note pass as a command of the conversation it reviews,
//! and the bookkeeping a conversation's own turns give the pass: see
//! `plugins::host::memory::note_pass`.

use std::sync::Arc;

use renoa_agent::ContentBlock;
use renoa_agent_loop::AgentCommand;
use renoa_kernel::{CommandId, Runtime};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{AgentSession, execution::SessionCommand, memory_job::Unwatched};
use crate::{
    LocalHostError, LocalTurnOutcome,
    plugins::{
        PluginError,
        host::memory::note_pass::{self, SessionRef},
    },
    session::PromptAdmission,
};

/// A pass's frozen message and the person whose conversation it reviews.
pub(super) struct NotePass {
    pub(super) content: Vec<ContentBlock>,
    pub(super) principal: Uuid,
}

impl AgentSession {
    /// Runs the note pass frozen under `request_id` with `prompt` for
    /// `principal`, or replays or settles the one already admitted under it.
    /// `cancellation` stops it: the kernel cancels an in-flight model call,
    /// and a pass cancelled before admission is never admitted.
    ///
    /// # Errors
    ///
    /// Returns request coordination, runtime resolution, admission, or
    /// execution failures; the pass's command may then be unfinished.
    pub(crate) async fn execute_note_pass(
        &self,
        request_id: Uuid,
        prompt: &str,
        principal: Uuid,
        cancellation: CancellationToken,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        self.execute(
            request_id,
            SessionCommand::NotePass(NotePass {
                content: vec![ContentBlock::text(prompt)],
                principal,
            }),
            Arc::new(Unwatched),
            cancellation,
        )
        .await
    }

    /// Admits a pass as a receipted prompt, with no time or context, or
    /// resumes the command already admitted under `command_id`, and drives it.
    pub(super) async fn admit_note_pass(
        &self,
        command_id: CommandId,
        pass: NotePass,
        runtime: &Runtime,
        cancellation: CancellationToken,
    ) -> Result<LocalTurnOutcome, LocalHostError> {
        let command = match self.kernel.prompt_admission(command_id, &pass.content)? {
            PromptAdmission::Admitted(command) => command,
            PromptAdmission::New { .. } => AgentCommand::receipted(pass.content),
        };
        Ok(self
            .kernel
            .execute_prompt(command_id, command, runtime, cancellation)
            .await?)
    }

    /// Closes a pass a stopped process left in this conversation, so the
    /// kernel admits the next command. A failure is logged, never returned:
    /// the person's message goes on, and the kernel refuses it only if the
    /// pass's command is still unfinished.
    pub(super) async fn close_stale_note_pass(&self) {
        if let Err(error) = note_pass::close_stale(self).await {
            note_pass::unrecorded(self.id, &error);
        }
    }

    /// After the person's message `request_id` ran: once the kernel admitted
    /// it, counts its characters, received at the time `said` carries,
    /// toward this conversation's unreviewed text, asks again when its pass
    /// is due, and restarts its quiet spell. A message the kernel refused
    /// counts for nothing, so one sent again counts once.
    pub(super) async fn note_message(&self, request_id: Uuid, said: (usize, i64), principal: Uuid) {
        let command = CommandId::from_uuid(request_id);
        match self.kernel.command_standing(command) {
            Ok(standing) if standing.admitted => {}
            Ok(_) => return,
            Err(error) => {
                note_pass::unrecorded(self.id, &PluginError::Invalid(error.to_string()));
                return;
            }
        }
        let at = self.pass_ref();
        let first = tokio::task::spawn_blocking(move || note_pass::first_followed(&at)).await;
        // The journal is read only for the first message the pass follows;
        // one that cannot be read counts as earlier turns, so nothing is
        // noted twice.
        let earlier = match first {
            Ok(Ok(true)) => {
                let kernel = Arc::clone(&self.kernel);
                tokio::task::spawn_blocking(move || kernel.history())
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .is_none_or(|turns| turns.iter().any(|turn| turn.command_id != command))
            }
            Ok(Ok(false)) => false,
            unread => {
                self.log_unrecorded(unread.map(|first| first.map(drop)));
                return;
            }
        };
        let at = self.pass_ref();
        let recorded = tokio::task::spawn_blocking(move || {
            note_pass::message(&at, principal, request_id, said, earlier)
        })
        .await;
        self.log_unrecorded(recorded);
        let at = self.pass_ref();
        let ended = tokio::task::spawn_blocking(move || note_pass::turn_ended(&at)).await;
        self.log_unrecorded(ended);
    }

    /// The messages of `command`, a receipted pass's included, read off the
    /// executor.
    ///
    /// # Errors
    ///
    /// Returns durable history corruption or storage failures.
    pub(crate) async fn command_messages(
        &self,
        command: Uuid,
    ) -> Result<Vec<renoa_agent::Message>, LocalHostError> {
        let command = CommandId::from_uuid(command);
        let kernel = Arc::clone(&self.kernel);
        let messages = tokio::task::spawn_blocking(move || kernel.all_messages()).await??;
        Ok(messages
            .into_iter()
            .filter(|entry| entry.command_id == command)
            .map(|entry| entry.message)
            .collect())
    }

    /// This conversation, as the pass's bookkeeping needs it.
    pub(crate) fn pass_ref(&self) -> SessionRef {
        SessionRef {
            database: self.host.database.clone(),
            data_directory: self.host.home.path().to_path_buf(),
            agent: self.agent,
            session: self.id,
            workspace: self.workspace.clone(),
        }
    }

    fn log_unrecorded(&self, recorded: Result<Result<(), PluginError>, tokio::task::JoinError>) {
        if let Err(error) = recorded.map_err(PluginError::from).flatten() {
            note_pass::unrecorded(self.id, &error);
        }
    }
}

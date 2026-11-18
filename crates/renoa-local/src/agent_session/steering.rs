//! Steering a running turn: a person's message that joins the turn of an
//! earlier command before its next model call, instead of waiting to start a
//! turn of its own.

use renoa_agent::ContentBlock;
use renoa_agent_loop::AgentCommand;
use renoa_kernel::{CommandId, SteerState};
use uuid::Uuid;

use super::AgentSession;
use crate::{LocalHostError, Speaker, TurnObservation, plugins::host::message_context};

impl AgentSession {
    /// Offers command `steer` to the turn of command `target`: its content
    /// joins that turn before its next model call, with the context a message
    /// of `speaker` gets.
    ///
    /// The caller decides that `speaker` may steer that turn. An offer of a
    /// steer already recorded returns its state without recomputing it, so
    /// retries converge. A steer the turn never takes ([`SteerState::Released`])
    /// runs as its own command afterward; one it took
    /// ([`SteerState::Accepted`]) must not ([`Self::steered`]).
    ///
    /// # Errors
    ///
    /// Returns a refusal when the target's turn already finished or `steer`
    /// already ran as a turn of its own, which leaves `steer` to run as an
    /// ordinary command, or a storage failure.
    pub async fn steer(
        &self,
        target: Uuid,
        steer: Uuid,
        content: Vec<ContentBlock>,
        observation: TurnObservation,
        speaker: Speaker,
    ) -> Result<SteerState, LocalHostError> {
        let steer = CommandId::from_uuid(steer);
        if let Some(state) = self.kernel.steer_state(steer)? {
            return Ok(state);
        }
        let database = self.host.database.clone();
        let agent = self.agent;
        let observed_at = observation.unix_milliseconds();
        // Reading plugin state is blocking catalog I/O. A contributor left out
        // is left out of this message only, as for any message.
        let (entries, _skipped) = tokio::task::spawn_blocking(move || {
            message_context::admit(
                &database,
                agent,
                speaker,
                observation.surface_context(),
                observed_at,
                None,
            )
        })
        .await?;
        let command = AgentCommand::observed(content, observed_at, entries)
            .map_err(crate::LocalSessionError::from)?;
        Ok(self
            .kernel
            .request_steer(CommandId::from_uuid(target), steer, command)?)
    }

    /// Whether command `command` was taken into an earlier turn as a steer,
    /// so it has no turn of its own.
    ///
    /// # Errors
    ///
    /// Returns a storage failure.
    pub fn steered(&self, command: Uuid) -> Result<bool, LocalHostError> {
        Ok(self.kernel.steer_state(CommandId::from_uuid(command))? == Some(SteerState::Accepted))
    }
}

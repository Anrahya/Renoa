//! Steers of this session's turns: messages that join a running turn before
//! its next model call (see `renoa_kernel::SteerState`).

use renoa_agent_loop::AgentCommand;
use renoa_kernel::{Command, CommandId, KernelError, SteerState};

use super::{LocalSession, LocalSessionError, execution::encode_command};

impl LocalSession {
    /// Where steer `steer` stands, or `None` when it was never recorded.
    ///
    /// # Errors
    ///
    /// Returns a kernel storage failure.
    pub fn steer_state(&self, steer: CommandId) -> Result<Option<SteerState>, LocalSessionError> {
        Ok(self.kernel.steer_state(steer)?)
    }

    /// Records `command` as steer `steer` of the turn of command `target`.
    /// A steer already recorded keeps what it was recorded with: its state
    /// is returned, and `command` is not compared.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::SteerTargetUnavailable`] when the target's turn
    /// finished, [`KernelError::CommandConflict`] when `steer` already ran as
    /// a turn of its own, or a storage failure.
    pub fn request_steer(
        &self,
        target: CommandId,
        steer: CommandId,
        command: AgentCommand,
    ) -> Result<SteerState, LocalSessionError> {
        if let Some(state) = self.steer_state(steer)? {
            return Ok(state);
        }
        let command = Command::new(steer, encode_command(command)?);
        match self.kernel.request_steer(self.session_id, target, command) {
            Ok(state) => Ok(state),
            // Recorded between the read above and this write.
            Err(KernelError::SteerConflict { .. }) => self
                .steer_state(steer)?
                .ok_or_else(|| KernelError::Corrupt("a conflicting steer is absent".to_owned()).into()),
            Err(error) => Err(error.into()),
        }
    }
}

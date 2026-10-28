//! Stopping a command the coordinator delivered a stop for.
//!
//! A stop is recorded in the node ledger before it is acknowledged. It then
//! reaches the command two ways: the running turn's token, if the command is
//! running, and the ledger, which a command reads when it starts. A command
//! registers its token before it reads the ledger and a stop writes the ledger
//! before it reads the registry, so whichever happens second sees the other.

use std::{
    collections::HashMap,
    sync::{Mutex, PoisonError},
};

use renoa_control::TaskId;
use renoa_protocol::CommandId;
use tokio_util::sync::CancellationToken;

use crate::{
    bridge::{NodeError, NodeRuntime},
    node_store::StopRecord,
};

/// The tokens of the turns this node is running, by command.
#[derive(Default)]
pub(crate) struct RunningTurns {
    tokens: Mutex<HashMap<CommandId, CancellationToken>>,
}

/// Keeps a command's token registered until the command stops running.
pub(crate) struct Registration<'a> {
    turns: &'a RunningTurns,
    command_id: CommandId,
}

impl RunningTurns {
    pub(crate) fn register(
        &self,
        command_id: CommandId,
        token: CancellationToken,
    ) -> Registration<'_> {
        self.tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(command_id, token);
        Registration {
            turns: self,
            command_id,
        }
    }

    /// Cancels the command's token, if the command is running.
    fn cancel(&self, command_id: CommandId) {
        if let Some(token) = self
            .tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&command_id)
        {
            token.cancel();
        }
    }
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        self.turns
            .tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.command_id);
    }
}

impl NodeRuntime {
    /// Records the stop, then cancels the command if it is running. The
    /// caller acknowledges the stop to the coordinator only after this
    /// returns, so an acknowledged stop is durable.
    pub(crate) async fn stop(
        &self,
        task_id: TaskId,
        command_id: CommandId,
    ) -> Result<StopRecord, NodeError> {
        let record = self.state.record_stop(task_id, command_id).await?;
        if record == StopRecord::Recorded {
            self.turns.cancel(command_id);
        }
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use renoa_protocol::CommandId;
    use tokio_util::sync::CancellationToken;

    use super::RunningTurns;

    #[test]
    fn cancelling_a_command_reaches_only_that_commands_token() {
        let turns = RunningTurns::default();
        let (stopped, other) = (CommandId::new(), CommandId::new());
        let (stopped_token, other_token) = (CancellationToken::new(), CancellationToken::new());
        let _stopped = turns.register(stopped, stopped_token.clone());
        let _other = turns.register(other, other_token.clone());

        turns.cancel(stopped);

        assert!(stopped_token.is_cancelled());
        assert!(!other_token.is_cancelled());
    }

    #[test]
    fn a_command_that_stopped_running_is_no_longer_reachable() {
        let turns = RunningTurns::default();
        let command_id = CommandId::new();
        let token = CancellationToken::new();
        drop(turns.register(command_id, token.clone()));

        turns.cancel(command_id);

        assert!(!token.is_cancelled());
    }
}

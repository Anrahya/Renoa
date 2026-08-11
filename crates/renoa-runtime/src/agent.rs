use renoa_core::{CommandEnvelope, Message, ResolvedAgent};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{AgentRunResult, Engine, EngineError};

#[derive(Serialize, Deserialize)]
pub struct AgentState {
    messages: Vec<Message>,
}

#[derive(Debug, Error)]
#[error("agent state cannot contain system messages")]
pub struct AgentStateError;

/// A stateful agent that carries one conversation across commands.
pub struct Agent {
    engine: Engine,
    definition: ResolvedAgent,
    state: AgentState,
}

impl Agent {
    #[must_use]
    pub fn new(engine: Engine, definition: ResolvedAgent) -> Self {
        Self {
            engine,
            definition,
            state: AgentState {
                messages: Vec::new(),
            },
        }
    }

    /// Restores host-persisted conversation state.
    ///
    /// # Errors
    ///
    /// Returns `AgentStateError` when state attempts to override the immutable
    /// system instructions supplied by the agent definition.
    pub fn from_state(
        engine: Engine,
        definition: ResolvedAgent,
        state: AgentState,
    ) -> Result<Self, AgentStateError> {
        if state
            .messages
            .iter()
            .any(|message| matches!(message, Message::System { .. }))
        {
            return Err(AgentStateError);
        }
        Ok(Self {
            engine,
            definition,
            state,
        })
    }

    #[must_use]
    pub fn state(&self) -> &AgentState {
        &self.state
    }

    /// Executes one command using the conversation produced by earlier calls.
    ///
    /// # Errors
    ///
    /// Returns `EngineError` under the same conditions as [`Engine::run`].
    pub async fn prompt(
        &mut self,
        command: CommandEnvelope,
        cancellation: CancellationToken,
    ) -> Result<AgentRunResult, EngineError> {
        self.engine
            .run_in_context(
                command,
                &self.definition,
                &mut self.state.messages,
                cancellation,
            )
            .await
    }
}

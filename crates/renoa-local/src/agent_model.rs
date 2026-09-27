use serde::{Deserialize, Serialize};

use crate::{AgentDefinitionError, ModelProvider, ReasoningLevel};

/// An agent's default model. Each conversation may select another enabled model.
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentModelSelection {
    pub provider: ModelProvider,
    pub model: String,
    pub reasoning: Option<ReasoningLevel>,
}

impl AgentModelSelection {
    pub(crate) fn validate(&self) -> Result<(), AgentDefinitionError> {
        if self.model.trim().is_empty()
            || self.model.len() > 512
            || self.model.chars().any(char::is_control)
        {
            return Err(AgentDefinitionError::InvalidModel);
        }
        Ok(())
    }
}

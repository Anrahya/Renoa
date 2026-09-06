use renoa_local::{AgentSession, ReasoningLevel};

use crate::{SlackError, commands::Command};

pub(crate) async fn run(session: &AgentSession, command: &Command) -> Result<String, SlackError> {
    match command {
        Command::Status => {
            let configuration = session.configuration()?;
            Ok(format!(
                "Agent: {}\nSession: {}\nModel: {}\nReasoning: {}\nLatest context: {:?} / {} tokens",
                session.agent_id(),
                session.id(),
                configuration.model,
                configuration.reasoning.name(),
                session.latest_context_tokens()?,
                session.context_window_tokens()?
            ))
        }
        Command::Model(requested) => {
            if let Some(requested) = requested {
                session.set_model(requested).await?;
            }
            let configuration = session.refresh_configuration().await?;
            let models = configuration
                .models
                .iter()
                .map(|model| format!("{} — {}", model.selection_id(), model.name()))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(format!(
                "Current model: {}\nReasoning: {}\n\nAvailable models:\n{models}\n\nUse !model <id> to change it.",
                configuration.model,
                configuration.reasoning.name()
            ))
        }
        Command::Reasoning(requested) => {
            if let Some(requested) = requested {
                let level = ReasoningLevel::from_id(requested)
                    .ok_or_else(|| SlackError::Invalid("unknown reasoning level".to_owned()))?;
                session.set_reasoning(level).await?;
            }
            let configuration = session.refresh_configuration().await?;
            let selected = configuration
                .models
                .iter()
                .find(|model| model.selection_id() == configuration.model)
                .ok_or_else(|| {
                    SlackError::Invalid("selected model is absent from the catalog".to_owned())
                })?;
            let levels = selected
                .reasoning_levels()
                .iter()
                .map(|level| level.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Ok(format!(
                "Current reasoning: {}\nAvailable: {levels}\nUse !reasoning <level> to change it.",
                configuration.reasoning.name()
            ))
        }
        _ => Err(SlackError::Invalid(
            "unsupported session control".to_owned(),
        )),
    }
}

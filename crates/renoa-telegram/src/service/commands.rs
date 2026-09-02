use renoa_local::{AgentSession, AgentSessionConfiguration, LocalHostError, ReasoningLevel};

pub(super) async fn model(
    session: &AgentSession,
    requested: Option<&str>,
) -> Result<String, LocalHostError> {
    if let Some(requested) = requested {
        session.set_model(requested).await?;
        let configuration = session.configuration()?;
        let selected = selected_model(&configuration)?;
        return Ok(format!(
            "Model changed to {} ({}).\nReasoning: {}.",
            selected.name(),
            selected.id(),
            configuration.reasoning.name()
        ));
    }
    let configuration = session.refresh_configuration().await?;
    let selected = selected_model(&configuration)?;
    let mut response = format!(
        "Current model: {} ({}).\n\nAvailable models:",
        selected.name(),
        selected.id()
    );
    for model in &configuration.models {
        response.push_str("\n- ");
        response.push_str(model.name());
        response.push_str(" — ");
        response.push_str(model.id());
    }
    response.push_str("\n\nUse /model <id> to change it.");
    Ok(response)
}

pub(super) async fn reasoning(
    session: &AgentSession,
    requested: Option<&str>,
) -> Result<String, LocalHostError> {
    let mut configuration = session.refresh_configuration().await?;
    if let Some(requested) = requested {
        let level = ReasoningLevel::from_id(&requested.to_ascii_lowercase()).ok_or_else(|| {
            LocalHostError::InvalidRequest(format!(
                "unknown reasoning level `{requested}`; use /reasoning to see this model's choices"
            ))
        })?;
        session.set_reasoning(level).await?;
        configuration = session.configuration()?;
    }
    let selected = selected_model(&configuration)?;
    if requested.is_some() {
        return Ok(format!(
            "Reasoning changed to {} for {}.",
            configuration.reasoning.name(),
            selected.name()
        ));
    }
    let levels = selected
        .reasoning_levels()
        .iter()
        .map(|level| level.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "Current reasoning: {}.\nAvailable for {}: {levels}.\n\nUse /reasoning <level> to change it.",
        configuration.reasoning.name(),
        selected.name()
    ))
}

fn selected_model(
    configuration: &AgentSessionConfiguration,
) -> Result<&renoa_local::ModelChoice, LocalHostError> {
    configuration
        .models
        .iter()
        .find(|model| model.selection_id() == configuration.model)
        .ok_or_else(|| {
            LocalHostError::Configuration(
                "active model is missing from the refreshed provider catalog".to_owned(),
            )
        })
}

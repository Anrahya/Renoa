use super::{HostConfig, LocalHostError};
use crate::selection::RuntimeSelection;
use crate::{AgentProfile, ModelChoice, ModelProvider, ReasoningLevel, discover_models};

pub(crate) async fn discover_profile_models(
    host: &HostConfig,
    profile: &AgentProfile,
) -> Result<Vec<ModelChoice>, LocalHostError> {
    if let Some(provider) = profile.model_provider()
        && !host.providers.contains(&provider)
    {
        return Err(LocalHostError::Configuration(format!(
            "profile `{}` requires the {} provider, but it is not enabled",
            profile.id(),
            provider.name()
        )));
    }
    let mut models = Vec::new();
    for provider in host.providers.iter().filter(|provider| {
        profile
            .model_provider()
            .is_none_or(|required| required == **provider)
    }) {
        models.extend(
            discover_models(
                host.bridge.clone(),
                *provider,
                host.credential_store.clone(),
            )
            .await?,
        );
    }
    Ok(models)
}

pub(crate) fn initial_reasoning(
    models: &[ModelChoice],
    provider: ModelProvider,
    configured_model: &str,
) -> Result<ReasoningLevel, LocalHostError> {
    let model = require_model(models, provider, configured_model, "configured")?;
    model.default_reasoning().ok_or_else(|| {
        LocalHostError::Configuration(format!(
            "configured {configured_model} model has no supported reasoning level"
        ))
    })
}

fn selected_model<'a>(
    models: &'a [ModelChoice],
    provider: ModelProvider,
    id: &str,
) -> Option<&'a ModelChoice> {
    models
        .iter()
        .find(|model| model.provider() == provider && model.id() == id)
}

pub(crate) fn selected_model_by_selection_id<'a>(
    models: &'a [ModelChoice],
    selection_id: &str,
) -> Option<&'a ModelChoice> {
    models
        .iter()
        .find(|model| model.selection_id() == selection_id)
}

pub(crate) fn require_model<'a>(
    models: &'a [ModelChoice],
    provider: ModelProvider,
    id: &str,
    source: &str,
) -> Result<&'a ModelChoice, LocalHostError> {
    selected_model(models, provider, id).ok_or_else(|| {
        LocalHostError::Configuration(format!(
            "{source} {provider}/{id} model is not available from the authenticated provider"
        ))
    })
}

pub(super) fn validate_selection<'a>(
    models: &'a [ModelChoice],
    selection: &RuntimeSelection,
) -> Result<&'a ModelChoice, LocalHostError> {
    let model = require_model(models, selection.provider, &selection.model, "saved")?;
    if !model.reasoning_levels().contains(&selection.reasoning) {
        return Err(LocalHostError::Configuration(format!(
            "saved {} model no longer supports {} reasoning",
            selection.model,
            selection.reasoning.as_str()
        )));
    }
    Ok(model)
}

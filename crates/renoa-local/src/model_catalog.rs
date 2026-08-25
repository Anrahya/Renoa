use std::{collections::HashSet, num::NonZeroU64, path::PathBuf};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::model_bridge::{ModelBridgeConfig, ModelBridgeError, decode_response, run_bridge};

/// A model provider supported by Renoa's native process adapter.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub enum ModelProvider {
    #[serde(rename = "xai")]
    Xai,
    #[serde(rename = "opencode-go")]
    OpenCodeGo,
}

impl ModelProvider {
    #[must_use]
    pub fn from_id(value: &str) -> Option<Self> {
        match value {
            "xai" => Some(Self::Xai),
            "opencode-go" => Some(Self::OpenCodeGo),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Xai => "xai",
            Self::OpenCodeGo => "opencode-go",
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Xai => "xAI",
            Self::OpenCodeGo => "OpenCode Go",
        }
    }
}

impl std::fmt::Display for ModelProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A reasoning level understood by the provider-neutral model API.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ReasoningLevel {
    #[must_use]
    pub fn from_id(value: &str) -> Option<Self> {
        match value {
            "off" => Some(Self::Off),
            "minimal" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::Xhigh),
            "max" => Some(Self::Max),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Minimal => "Minimal",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Xhigh => "Extra High",
            Self::Max => "Max",
        }
    }
}

/// One model that the configured provider can resolve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelChoice {
    provider: ModelProvider,
    id: String,
    name: String,
    reasoning_levels: Vec<ReasoningLevel>,
    context_window_tokens: NonZeroU64,
    model_spec: serde_json::Value,
}

impl ModelChoice {
    #[must_use]
    pub const fn provider(&self) -> ModelProvider {
        self.provider
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Opaque surface identity. Provider qualification prevents collisions
    /// such as `grok-4.5`, which exists in both supported catalogs.
    #[must_use]
    pub fn selection_id(&self) -> String {
        format!("{}/{}", self.provider.as_str(), self.id)
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &self.reasoning_levels
    }

    #[must_use]
    pub const fn context_window_tokens(&self) -> NonZeroU64 {
        self.context_window_tokens
    }

    /// The Host-owned default for a newly selected model.
    #[must_use]
    pub fn default_reasoning(&self) -> Option<ReasoningLevel> {
        self.reasoning_levels
            .contains(&ReasoningLevel::High)
            .then_some(ReasoningLevel::High)
            .or_else(|| self.reasoning_levels.first().copied())
    }

    pub(crate) fn encoded_spec(&self) -> String {
        serde_json::to_string(&self.model_spec)
            .expect("a parsed JSON model specification always serializes")
    }
}

/// Discovers the authenticated provider's current model choices.
///
/// # Errors
///
/// Returns an error when provider configuration, authentication, or the bridge response is invalid.
pub async fn discover_models(
    bridge: impl Into<PathBuf>,
    provider: ModelProvider,
    credential_store: impl Into<PathBuf>,
) -> Result<Vec<ModelChoice>, ModelBridgeError> {
    let config = ModelBridgeConfig::for_provider(bridge, provider.as_str(), credential_store)?;
    let encoded = run_bridge(
        config,
        "catalog",
        None,
        Vec::new(),
        CancellationToken::new(),
    )
    .await
    .map_err(ModelBridgeError::ModelResolution)?;
    let catalog: BridgeModelCatalog =
        decode_response(&encoded).map_err(ModelBridgeError::ModelResolution)?;
    validate_catalog(provider, catalog.models)
}

#[derive(Deserialize)]
struct BridgeModelCatalog {
    models: Vec<BridgeModelChoice>,
}

#[derive(Deserialize)]
struct BridgeModelChoice {
    id: String,
    name: String,
    reasoning_levels: Vec<ReasoningLevel>,
    context_window_tokens: u64,
    model_spec: serde_json::Value,
}

fn validate_catalog(
    provider: ModelProvider,
    models: Vec<BridgeModelChoice>,
) -> Result<Vec<ModelChoice>, ModelBridgeError> {
    let mut ids = HashSet::with_capacity(models.len());
    for model in &models {
        let levels = model
            .reasoning_levels
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        if model.id.is_empty()
            || model.name.is_empty()
            || model.reasoning_levels.is_empty()
            || levels.len() != model.reasoning_levels.len()
            || model
                .model_spec
                .get("id")
                .and_then(serde_json::Value::as_str)
                != Some(model.id.as_str())
            || !ids.insert(model.id.as_str())
        {
            return Err(ModelBridgeError::InvalidModelCatalog);
        }
    }
    if models.is_empty() {
        return Err(ModelBridgeError::InvalidModelCatalog);
    }
    models
        .into_iter()
        .map(|model| {
            let context_window_tokens = NonZeroU64::new(model.context_window_tokens)
                .ok_or(ModelBridgeError::InvalidModelCatalog)?;
            Ok(ModelChoice {
                provider,
                id: model.id,
                name: model.name,
                reasoning_levels: model.reasoning_levels,
                context_window_tokens,
                model_spec: model.model_spec,
            })
        })
        .collect()
}

use std::{collections::HashSet, path::PathBuf};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::model_bridge::{ModelBridgeConfig, ModelBridgeError, decode_response, run_bridge};

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
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct ModelChoice {
    id: String,
    name: String,
    reasoning_levels: Vec<ReasoningLevel>,
    model_spec: serde_json::Value,
}

impl ModelChoice {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &self.reasoning_levels
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
    provider: impl Into<String>,
    credential_store: impl Into<PathBuf>,
) -> Result<Vec<ModelChoice>, ModelBridgeError> {
    let config = ModelBridgeConfig::for_provider(bridge, provider, credential_store)?;
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
    validate_catalog(catalog.models)
}

#[derive(Deserialize)]
struct BridgeModelCatalog {
    models: Vec<ModelChoice>,
}

fn validate_catalog(models: Vec<ModelChoice>) -> Result<Vec<ModelChoice>, ModelBridgeError> {
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
    Ok(models)
}

use std::{collections::HashSet, path::PathBuf};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::pi_model::{PiBridgeConfig, PiModelConfigError, decode_response, run_bridge};

/// A reasoning level understood by Pi's provider-neutral model API.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PiReasoningLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl PiReasoningLevel {
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

/// One model that the configured Pi provider can resolve.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct PiModelOption {
    id: String,
    name: String,
    reasoning_levels: Vec<PiReasoningLevel>,
    model_spec: serde_json::Value,
}

impl PiModelOption {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn reasoning_levels(&self) -> &[PiReasoningLevel] {
        &self.reasoning_levels
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
pub async fn discover_pi_models(
    bridge: impl Into<PathBuf>,
    provider: impl Into<String>,
    credential_store: impl Into<PathBuf>,
) -> Result<Vec<PiModelOption>, PiModelConfigError> {
    let config = PiBridgeConfig::for_provider(bridge, provider, credential_store)?;
    let encoded = run_bridge(
        config,
        "catalog",
        None,
        Vec::new(),
        CancellationToken::new(),
    )
    .await
    .map_err(PiModelConfigError::ModelResolution)?;
    let catalog: PiModelCatalog =
        decode_response(&encoded).map_err(PiModelConfigError::ModelResolution)?;
    validate_catalog(catalog.models)
}

#[derive(Deserialize)]
struct PiModelCatalog {
    models: Vec<PiModelOption>,
}

fn validate_catalog(models: Vec<PiModelOption>) -> Result<Vec<PiModelOption>, PiModelConfigError> {
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
            return Err(PiModelConfigError::InvalidModelCatalog);
        }
    }
    if models.is_empty() {
        return Err(PiModelConfigError::InvalidModelCatalog);
    }
    Ok(models)
}

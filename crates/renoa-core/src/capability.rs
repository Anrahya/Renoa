use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{BoxFuture, RunId, TargetRef};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityCall {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityRequest {
    pub run_id: RunId,
    pub target: TargetRef,
    pub ordinal: u32,
    pub call: CapabilityCall,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityOutcome {
    pub model_view: Value,
    pub is_error: bool,
}

impl CapabilityOutcome {
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            model_view: serde_json::json!({ "error": message }),
            is_error: true,
        }
    }
}

/// Executes named capabilities against a target environment.
///
/// Implementations encode failures as `CapabilityOutcome` values so the model
/// can inspect the error and recover on the next round.
pub trait CapabilityHost: Send + Sync {
    fn specs(&self) -> Vec<CapabilitySpec>;

    fn execute(
        &self,
        request: CapabilityRequest,
        cancellation: CancellationToken,
    ) -> BoxFuture<'_, CapabilityOutcome>;
}

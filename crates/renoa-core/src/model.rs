use futures_util::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{CapabilityCall, CapabilityOutcome, CapabilitySpec, ModelError, RunId};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    System {
        text: String,
    },
    User {
        text: String,
    },
    Assistant {
        text: String,
        capability_calls: Vec<CapabilityCall>,
    },
    Capability {
        call_id: String,
        name: String,
        outcome: CapabilityOutcome,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelResponse {
    pub text: String,
    pub capability_calls: Vec<CapabilityCall>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRequest {
    pub run_id: RunId,
    pub round: u32,
    pub messages: Vec<Message>,
    pub capabilities: Vec<CapabilitySpec>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModelEvent {
    TextDelta { text: String },
    Completed { response: ModelResponse },
}

pub type ModelEventStream<'a> = BoxStream<'a, Result<ModelEvent, ModelError>>;

/// Streams one provider-neutral model invocation.
///
/// On success, drivers emit zero or more deltas followed by one `Completed`
/// event. They must honor the cancellation token as promptly as their
/// transport permits.
pub trait ModelDriver: Send + Sync {
    fn stream(
        &self,
        request: ModelRequest,
        cancellation: CancellationToken,
    ) -> ModelEventStream<'_>;
}

//! Owner Host control calls the coordinator relays to this node.
//!
//! Calls run beside the node's own work and never hold up its publication
//! loop. A call outlives neither its connection nor the node: the owner's
//! browser retries a change under the same operation identity, and the Host
//! answers from the receipt it recorded. A call whose handler panics is never
//! answered, so the coordinator's deadline ends it.

use std::sync::Arc;

use renoa_control::{ClientMessage, HostMethod};
use renoa_protocol::PrincipalId;
use tokio::task::JoinSet;
use uuid::Uuid;

use super::MAX_APPLICATION_MESSAGE_BYTES;
use crate::{bridge::NodeRuntime, host_control::Reply, node_log};

pub(super) struct IncomingCall {
    pub(super) call_id: Uuid,
    pub(super) principal_id: PrincipalId,
    pub(super) method: HostMethod,
    pub(super) path: String,
    pub(super) body: Option<String>,
}

pub(super) struct HostCalls {
    runtime: Arc<NodeRuntime>,
    running: JoinSet<ClientMessage>,
}

impl HostCalls {
    pub(super) fn new(runtime: Arc<NodeRuntime>) -> Self {
        Self {
            runtime,
            running: JoinSet::new(),
        }
    }

    pub(super) fn is_idle(&self) -> bool {
        self.running.is_empty()
    }

    pub(super) fn start(&mut self, call: IncomingCall) {
        let runtime = Arc::clone(&self.runtime);
        self.running.spawn(async move {
            let reply = runtime
                .host_control
                .handle(
                    call.principal_id,
                    call.method,
                    &call.path,
                    call.body.as_deref(),
                )
                .await;
            answer(call.call_id, reply)
        });
    }

    /// The next finished call's answer. Only polled while a call runs.
    pub(super) async fn finished(&mut self) -> Option<ClientMessage> {
        match self.running.join_next().await? {
            Ok(message) => Some(message),
            Err(error) => {
                node_log::event(
                    "error",
                    "host_call_failed",
                    &serde_json::json!({ "error": error.to_string() }),
                );
                None
            }
        }
    }
}

/// The answer message, or a fixed one when the real answer cannot be relayed.
fn answer(call_id: Uuid, reply: Reply) -> ClientMessage {
    let message = ClientMessage::CompleteHostCall {
        call_id,
        status: reply.status,
        body: reply.body,
    };
    match serde_json::to_vec(&message) {
        Ok(encoded) if encoded.len() <= MAX_APPLICATION_MESSAGE_BYTES => message,
        _ => {
            let reply = Reply::too_large();
            ClientMessage::CompleteHostCall {
                call_id,
                status: reply.status,
                body: reply.body,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_too_large_for_one_message_is_replaced_by_a_fixed_one() {
        let large = Reply {
            status: 200,
            body: "x".repeat(MAX_APPLICATION_MESSAGE_BYTES),
        };
        let ClientMessage::CompleteHostCall { status, body, .. } = answer(Uuid::new_v4(), large)
        else {
            panic!("an answer is a completion");
        };
        assert_eq!(status, 502);
        assert!(body.contains("host_reply_too_large"));

        let fits = Reply {
            status: 200,
            body: "{}".to_owned(),
        };
        let ClientMessage::CompleteHostCall { status, body, .. } = answer(Uuid::new_v4(), fits)
        else {
            panic!("an answer is a completion");
        };
        assert_eq!((status, body.as_str()), (200, "{}"));
    }
}

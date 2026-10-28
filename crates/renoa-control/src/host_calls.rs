//! Owner calls to the Host, relayed to the node that serves it.
//!
//! A call is an HTTP-shaped request and reply under `/v1/host`, from a
//! browser over HTTP (`host_control_http`) or from a signed-in app or browser
//! over RCP (`host_control_rcp`). Both share this relay. It is not a
//! task: it enters no journal and the coordinator writes neither its request
//! body nor its answer to disk. It is held in memory only while pending, ends
//! when the node's connection does, and is never retried here. A change is
//! retried by its caller under the operation identity the Host already
//! records.

use std::{
    collections::{HashMap, hash_map::Entry},
    sync::{Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use renoa_protocol::PrincipalId;
use tokio::{sync::oneshot, time::timeout};
use uuid::Uuid;

use crate::{
    ControlError, HostMethod, NodeId, ServerMessage, control_log, coordinator::CoordinatorState,
};

pub(crate) const HOST_CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// The largest body any Host operation accepts; the node applies each
/// operation's own, smaller limit.
pub(crate) const MAX_BODY_BYTES: usize = 64 * 1024;

/// What the owner reads when the Host's answer cannot be passed on.
pub(crate) const INVALID_REPLY: &str =
    "Your Host sent an answer that could not be passed on. Retry in a moment.";

/// What the owner reads when the coordinator's own records fail them.
pub(crate) const HOST_UNREACHABLE: &str = "Your Host could not be reached. Retry in a moment.";

/// Whether an answer may be passed on: a result or a refusal. An
/// informational or redirect status would send the caller somewhere the Host
/// never named.
pub(crate) const fn relayable(status: u16) -> bool {
    matches!(status, 200..=299 | 400..=599)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct HostReply {
    pub(crate) status: u16,
    pub(crate) body: String,
}

pub(crate) struct HostRequest {
    pub(crate) method: HostMethod,
    pub(crate) path: String,
    pub(crate) body: Option<String>,
}

pub(crate) enum HostCallError {
    /// The principal owns no node, so it owns no Host.
    NotAnOwner,
    /// None of the owner's nodes is connected and serving Host control.
    Offline,
    /// Several of the owner's nodes serve Host control at once.
    Ambiguous,
    /// The serving node's connection ended before it answered.
    Disconnected,
    TimedOut,
    Store(ControlError),
}

impl HostCallError {
    /// What the owner reads, whichever way the call came. It names none of
    /// the machinery behind the Host.
    pub(crate) const fn message(&self) -> &'static str {
        match self {
            Self::NotAnOwner => "This login does not own a Host.",
            Self::Offline => "Your Host is not connected. Retry when it returns.",
            Self::Ambiguous => "More than one Host of yours is connected. Disconnect all but one.",
            Self::Disconnected => {
                "Your Host disconnected before it answered. If this was a change, retry the same request to recover its outcome."
            }
            Self::TimedOut => {
                "Your Host did not answer in time. If this was a change, retry the same request to recover its outcome."
            }
            Self::Store(_) => HOST_UNREACHABLE,
        }
    }
}

#[derive(Default)]
pub(crate) struct HostCalls {
    pending: Mutex<HashMap<Uuid, Pending>>,
}

struct Pending {
    connection_id: Uuid,
    reply: oneshot::Sender<HostReply>,
}

/// Removes its call from the pending set however the caller ends: an answer,
/// a deadline, a dropped browser connection.
struct Registration<'a> {
    calls: &'a HostCalls,
    call_id: Uuid,
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        self.calls.lock().remove(&self.call_id);
    }
}

impl HostCalls {
    fn lock(&self) -> MutexGuard<'_, HashMap<Uuid, Pending>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn register(
        &self,
        call_id: Uuid,
        connection_id: Uuid,
        reply: oneshot::Sender<HostReply>,
    ) -> Registration<'_> {
        self.lock().insert(
            call_id,
            Pending {
                connection_id,
                reply,
            },
        );
        Registration {
            calls: self,
            call_id,
        }
    }

    /// Delivers a node's answer to its caller. Only the connection the call
    /// was sent on may answer it; a late, repeated or foreign answer is
    /// dropped. Returns whether a caller was waiting for it.
    pub(crate) fn complete(&self, connection_id: Uuid, call_id: Uuid, reply: HostReply) -> bool {
        let pending = match self.lock().entry(call_id) {
            Entry::Occupied(entry) if entry.get().connection_id == connection_id => entry.remove(),
            Entry::Occupied(_) | Entry::Vacant(_) => return false,
        };
        pending.reply.send(reply).is_ok()
    }

    /// Fails every call sent on a connection that has ended.
    pub(crate) fn end_connection(&self, connection_id: Uuid) {
        self.lock()
            .retain(|_, pending| pending.connection_id != connection_id);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().len()
    }
}

/// Sends one call to the principal's Host node and waits for its answer.
pub(crate) async fn call(
    state: &CoordinatorState,
    principal_id: PrincipalId,
    request: HostRequest,
) -> Result<HostReply, HostCallError> {
    let owned = state
        .store
        .owned_nodes(principal_id)
        .await
        .map_err(HostCallError::Store)?;
    if owned.is_empty() {
        return Err(HostCallError::NotAnOwner);
    }
    dispatch(state, &owned, principal_id, request).await
}

/// Finds the one connected node of `owned` that serves Host control and sends
/// it the call. The call is registered while the node table is held, and a
/// closing connection ends its calls under that same lock
/// (`wire::cleanup_connection`), so a connection either ends the call or is
/// already gone from the table: no call waits on a link that has ended.
pub(crate) async fn dispatch(
    state: &CoordinatorState,
    owned: &[NodeId],
    principal_id: PrincipalId,
    request: HostRequest,
) -> Result<HostReply, HostCallError> {
    let call_id = Uuid::new_v4();
    let (reply, answer) = oneshot::channel();
    let (node_id, outgoing, _registration) = {
        let nodes = state.nodes.lock().await;
        let mut serving = owned.iter().filter_map(|node_id| {
            nodes
                .get(node_id)
                .filter(|node| node.host_control)
                .map(|node| (*node_id, node))
        });
        let (node_id, node) = serving.next().ok_or(HostCallError::Offline)?;
        if serving.next().is_some() {
            return Err(HostCallError::Ambiguous);
        }
        let registration = state
            .host_calls
            .register(call_id, node.connection_id, reply);
        (node_id, node.outgoing.clone(), registration)
    };
    control_log::event(
        "info",
        "host_call_sent",
        &serde_json::json!({
            "call_id": call_id,
            "node_id": node_id,
            "method": request.method,
            "path": request.path,
        }),
    );
    let message = ServerMessage::HostCall {
        call_id,
        principal_id,
        method: request.method,
        path: request.path,
        body: request.body,
    };
    let outcome = timeout(HOST_CALL_TIMEOUT, async {
        outgoing
            .send(message)
            .await
            .map_err(|_| HostCallError::Disconnected)?;
        answer.await.map_err(|_| HostCallError::Disconnected)
    })
    .await;
    outcome.unwrap_or(Err(HostCallError::TimedOut))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(body: &str) -> HostReply {
        HostReply {
            status: 200,
            body: body.to_owned(),
        }
    }

    #[test]
    fn only_the_connection_a_call_was_sent_on_may_answer_it() {
        let calls = HostCalls::default();
        let (call_id, sent_on, elsewhere) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let (sender, mut answer) = oneshot::channel();
        let _registration = calls.register(call_id, sent_on, sender);

        assert!(!calls.complete(elsewhere, call_id, reply("foreign")));
        assert!(
            answer.try_recv().is_err(),
            "a foreign answer is not delivered"
        );
        assert!(calls.complete(sent_on, call_id, reply("own")));
        assert_eq!(answer.try_recv().expect("delivered"), reply("own"));
        assert!(
            !calls.complete(sent_on, call_id, reply("again")),
            "a call is answered once"
        );
    }

    #[test]
    fn a_registration_ends_with_its_caller_and_a_connection_ends_its_calls() {
        let calls = HostCalls::default();
        let (kept, ended) = (Uuid::new_v4(), Uuid::new_v4());
        let (sender, _answer) = oneshot::channel();
        let registration = calls.register(Uuid::new_v4(), kept, sender);
        let (sender, answer) = oneshot::channel();
        let _other = calls.register(Uuid::new_v4(), ended, sender);
        assert_eq!(calls.len(), 2);

        calls.end_connection(ended);
        assert_eq!(calls.len(), 1);
        assert!(
            answer.blocking_recv().is_err(),
            "the caller of an ended connection stops waiting"
        );
        drop(registration);
        assert_eq!(calls.len(), 0);
    }
}

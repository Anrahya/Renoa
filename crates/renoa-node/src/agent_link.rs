//! The node's two requests on behalf of its agents: who they can message, and
//! the message itself.
//!
//! A request travels over the node's own coordinator session, so the
//! coordinator knows which node speaks. Nothing queues across a reconnect: a
//! request made while the session is down fails at once. A message in flight
//! when the session ends reports that its outcome is unknown; a roster lookup
//! changes nothing, so it only reports that it failed. Every wait ends after
//! [`REQUEST_TIMEOUT`].

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use renoa_agent::BoxFuture;
use renoa_control::{ClientMessage, ErrorCode, NodeId, TargetSummary};
use renoa_kernel::AgentId;
use renoa_local::{AgentPeers, PeerAgent, PeerError, PeerMessage};
use renoa_protocol::{CommandId, TargetRef};
use tokio::{
    sync::{mpsc, oneshot},
    time::timeout,
};
use uuid::Uuid;

use crate::{agent_targets, bridge::NodeError};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const QUEUE: usize = 16;

/// What the coordinator answered a request.
pub(crate) enum Answer {
    Targets(Vec<TargetSummary>),
    Accepted,
    Refused { code: ErrorCode, message: String },
}

enum Call {
    ListTargets,
    Send {
        from_command: CommandId,
        message_id: CommandId,
        node_id: NodeId,
        target: TargetRef,
        text: String,
    },
}

struct Request {
    call: Call,
    answer: oneshot::Sender<Answer>,
}

/// The place a live coordinator session accepts requests.
#[derive(Default)]
pub(crate) struct AgentLink {
    session: Mutex<Option<mpsc::Sender<Request>>>,
}

impl AgentLink {
    /// Starts accepting requests for one coordinator session. Dropping the
    /// returned value stops them and ends the wait of every request in flight.
    pub(crate) fn open(self: &Arc<Self>) -> LinkSession {
        let (sender, requests) = mpsc::channel(QUEUE);
        *self.session.lock().unwrap_or_else(PoisonError::into_inner) = Some(sender.clone());
        LinkSession {
            link: Arc::clone(self),
            sender,
            requests,
            waiting: HashMap::new(),
            next: 1,
        }
    }

    async fn call(&self, call: Call) -> Result<Answer, PeerError> {
        let delivers = matches!(call, Call::Send { .. });
        let sender = self
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or_else(|| {
                PeerError::Unavailable(
                    "Renoa cannot reach the other agents right now because this machine has lost its connection."
                        .to_owned(),
                )
            })?;
        let (answer, waiting) = oneshot::channel();
        sender.send(Request { call, answer }).await.map_err(|_| {
            PeerError::Unavailable(
                "The connection was lost before anything was sent, so nothing was delivered."
                    .to_owned(),
            )
        })?;
        match timeout(REQUEST_TIMEOUT, waiting).await {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(_)) => Err(no_answer(
                delivers,
                "the connection was lost while it was waiting",
            )),
            Err(_) => Err(no_answer(delivers, "no answer came in time")),
        }
    }
}

/// A request whose answer never came. A message may have been admitted, so
/// its outcome is unknown. A roster lookup changes nothing, so it only failed.
fn no_answer(delivers: bool, reason: &str) -> PeerError {
    if delivers {
        PeerError::Unconfirmed(format!(
            "It is not known whether the message was delivered, because {reason}. Sending it again may show the other agent the message twice."
        ))
    } else {
        PeerError::Unavailable(format!(
            "The list of agents could not be fetched, because {reason}."
        ))
    }
}

/// One coordinator session's side of the link.
pub(crate) struct LinkSession {
    link: Arc<AgentLink>,
    sender: mpsc::Sender<Request>,
    requests: mpsc::Receiver<Request>,
    waiting: HashMap<u64, oneshot::Sender<Answer>>,
    next: u64,
}

impl LinkSession {
    /// The next request to send, skipping any whose caller has stopped
    /// waiting. The message to send carries the request id its answer will
    /// name.
    pub(crate) async fn next_message(&mut self) -> ClientMessage {
        loop {
            // This session holds a sender, so the queue never closes.
            let Some(request) = self.requests.recv().await else {
                return std::future::pending().await;
            };
            if request.answer.is_closed() {
                continue;
            }
            let request_id = self.next;
            self.next += 1;
            self.waiting.insert(request_id, request.answer);
            return match request.call {
                Call::ListTargets => ClientMessage::ListTargets { request_id },
                Call::Send {
                    from_command,
                    message_id,
                    node_id,
                    target,
                    text,
                } => ClientMessage::SendAgentMessage {
                    request_id,
                    from_command,
                    message_id,
                    node_id,
                    target,
                    text,
                },
            };
        }
    }

    /// Hands the coordinator's answer to the request that asked.
    ///
    /// # Errors
    ///
    /// Fails when this session never issued `request_id`.
    pub(crate) fn answer(&mut self, request_id: u64, answer: Answer) -> Result<(), NodeError> {
        let waiting = self.waiting.remove(&request_id).ok_or_else(|| {
            NodeError::Protocol(format!(
                "coordinator answered request {request_id}, which this node did not make"
            ))
        })?;
        // The caller may have stopped waiting; that is its timeout.
        let _ = waiting.send(answer);
        Ok(())
    }
}

impl Drop for LinkSession {
    fn drop(&mut self) {
        let mut session = self
            .link
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if session
            .as_ref()
            .is_some_and(|current| current.same_channel(&self.sender))
        {
            *session = None;
        }
    }
}

/// The Host's view of the link: its agents' roster and messages.
pub(crate) struct Peers(pub(crate) Arc<AgentLink>);

impl AgentPeers for Peers {
    fn roster(&self, agent: AgentId) -> BoxFuture<'_, Result<Vec<PeerAgent>, PeerError>> {
        Box::pin(async move {
            let own = agent_targets::target(agent);
            match self.0.call(Call::ListTargets).await? {
                Answer::Targets(targets) => {
                    let mut roster = targets
                        .into_iter()
                        .filter(|summary| summary.target != own)
                        .map(|summary| PeerAgent {
                            name: summary.name,
                            route: format!("{}/{}", summary.node_id, summary.target.as_str()),
                        })
                        .collect::<Vec<_>>();
                    roster.sort_by(|left, right| {
                        (left.name.as_str(), left.route.as_str())
                            .cmp(&(right.name.as_str(), right.route.as_str()))
                    });
                    Ok(roster)
                }
                Answer::Accepted => Err(PeerError::Unavailable(
                    "Renoa gave an answer that was not the list of agents.".to_owned(),
                )),
                Answer::Refused { message, .. } => Err(PeerError::Refused(message)),
            }
        })
    }

    fn send(&self, message: PeerMessage) -> BoxFuture<'_, Result<(), PeerError>> {
        Box::pin(async move {
            let unknown = || {
                PeerError::Refused(
                    "Renoa does not recognise that agent. Look up the agents again and choose a name from the list."
                        .to_owned(),
                )
            };
            let (node, target) = message.to.route.split_once('/').ok_or_else(unknown)?;
            let node_id = Uuid::parse_str(node)
                .map(NodeId::from_uuid)
                .map_err(|_| unknown())?;
            let answer = self
                .0
                .call(Call::Send {
                    from_command: CommandId::from_uuid(message.from_command),
                    message_id: CommandId::from_uuid(message.message_id),
                    node_id,
                    target: TargetRef::new(target),
                    text: message.text,
                })
                .await?;
            match answer {
                Answer::Accepted => Ok(()),
                Answer::Refused { code, message } => Err(match code {
                    ErrorCode::NodeOffline => PeerError::Unavailable(message),
                    ErrorCode::Internal => PeerError::Unavailable(
                        "Renoa had a problem delivering the message.".to_owned(),
                    ),
                    _ => PeerError::Refused(message),
                }),
                Answer::Targets(_) => Err(PeerError::Unavailable(
                    "Renoa gave an answer that did not say whether the message was accepted."
                        .to_owned(),
                )),
            }
        })
    }
}

#[cfg(test)]
mod tests;

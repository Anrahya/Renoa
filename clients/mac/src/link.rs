//! The app's link to Renoa, run on a tokio runtime beside GPUI's own
//! executor. Views send it commands and read its updates over channels.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use renoa_protocol::{CommandId, LiveActivity, TargetRef};
pub use renoa_rcp_client::StopOutcome;
use renoa_rcp_client::{ClientError, Connection, connect_with_key};
use renoa_rcp_wire::{DeviceCredentials, ErrorCode, NodeId, TargetSummary, TaskEvent, TaskId};
use tokio::{
    runtime::Runtime,
    sync::{Notify, mpsc, oneshot},
    time::{MissedTickBehavior, sleep},
};
use uuid::Uuid;

use crate::{
    face::Look,
    key::MacKey,
    notice::{HeldLink, NoticeSecret},
};
use host::HostCall;
use notices::Notices;
use session::{
    Aborting, Followed, Outgoing, Through, flush, follow, forward, note, request_stop, within,
};
pub use session::{Endpoints, Pace, StopProblem};
pub use sign_in::{SignInStep, sign_in};

pub fn runtime() -> &'static Runtime {
    static RUNTIME: LazyLock<Runtime> =
        LazyLock::new(|| Runtime::new().expect("start the tokio runtime"));
    &RUNTIME
}

/// What the session tells the views.
#[derive(Debug, Clone)]
pub enum Update {
    Linked(bool),
    Agents(Vec<TargetSummary>),
    /// What the agent `target` on `node_id` is for, as its owner wrote it.
    Purpose {
        node_id: NodeId,
        target: String,
        purpose: String,
    },
    /// How the agent `target` on `node_id` looks, as the Host picked it.
    Look {
        node_id: NodeId,
        target: String,
        look: Look,
    },
    /// The Host's main agent, by its Host agent id; `None` while it names none.
    Main(Option<Uuid>),
    Event(Box<TaskEvent>),
    Live {
        task_id: TaskId,
        command_id: CommandId,
        activity: LiveActivity,
    },
    /// Renoa refused a message to this chat; it was not sent.
    NotSent {
        task_id: TaskId,
        text: String,
    },
    /// Unexpired setup links waiting for the owner, soonest to expire first.
    SetupLinks(Vec<HeldLink>),
    /// Renoa no longer knows this Mac: it was signed out.
    SignedOut,
}

/// What the views ask of the session.
#[derive(Debug)]
pub enum Command {
    /// Opens the chat task and follows it from after `after`.
    Follow {
        task_id: TaskId,
        node_id: NodeId,
        target: TargetRef,
        after: Option<u64>,
    },
    Send {
        task_id: TaskId,
        text: String,
        /// Joins the turn the agent is running, before its next model call,
        /// instead of waiting for a turn of its own (`false`). With no turn
        /// running, it runs as any message does.
        steer: bool,
        /// Told whether Renoa took the message (`true`) or refused it; it
        /// is kept across reconnects until one or the other.
        sent: Option<oneshot::Sender<bool>>,
    },
    /// Calls the Host, beside the chats, and answers on the call.
    Host(HostCall),
    /// Requests a one-time link to a shared file and answers with the full URL
    /// on the coordinator's origin, or `None` if Renoa refused or could not be reached.
    ShareLink {
        share_id: Uuid,
        answered: oneshot::Sender<Option<String>>,
    },
    /// Asks Renoa to stop executing `command_id` on `task_id`.
    Stop {
        task_id: TaskId,
        command_id: CommandId,
        answered: oneshot::Sender<Result<StopOutcome, StopProblem>>,
    },
    /// Acknowledges a setup notice the owner acted on or dismissed; answers
    /// whether Renoa recorded it.
    AcknowledgeNotice {
        notice_id: Uuid,
        answered: oneshot::Sender<bool>,
    },
}

enum Immediate {
    Host(HostCall),
    ShareLink {
        share_id: Uuid,
        answered: oneshot::Sender<Option<String>>,
    },
    Stop {
        task_id: TaskId,
        command_id: CommandId,
        answered: oneshot::Sender<Result<StopOutcome, StopProblem>>,
    },
    AcknowledgeNotice {
        notice_id: Uuid,
        answered: oneshot::Sender<bool>,
    },
}

/// What the session keeps across connections.
#[derive(Default)]
struct Kept {
    through: Through,
    followed: HashMap<TaskId, Followed>,
    outbox: VecDeque<Outgoing>,
    profiles: Arc<Mutex<profiles::Read>>,
}

impl Kept {
    /// Keeps a chat to follow or a message to send for the next catch-up;
    /// Host calls, share links, stops, and notice acks belong to the current connection.
    fn hold(&mut self, command: Command) -> Option<Immediate> {
        match command {
            Command::Follow {
                task_id,
                node_id,
                target,
                after,
            } => {
                if let Some(after) = after {
                    note(&self.through, task_id, after);
                }
                self.followed.entry(task_id).or_insert(Followed {
                    node_id,
                    target,
                    attached: false,
                });
                None
            }
            Command::Send {
                task_id,
                text,
                steer,
                sent,
            } => {
                self.outbox.push_back(Outgoing {
                    task_id,
                    command_id: CommandId::new(),
                    text,
                    steer,
                    sent,
                });
                None
            }
            Command::Host(call) => Some(Immediate::Host(call)),
            Command::ShareLink { share_id, answered } => {
                Some(Immediate::ShareLink { share_id, answered })
            }
            Command::Stop {
                task_id,
                command_id,
                answered,
            } => Some(Immediate::Stop {
                task_id,
                command_id,
                answered,
            }),
            Command::AcknowledgeNotice {
                notice_id,
                answered,
            } => Some(Immediate::AcknowledgeNotice {
                notice_id,
                answered,
            }),
        }
    }
}

/// Keeps one connection up for as long as the views hold the command
/// sender. After a reconnect it follows every chat again from the last event
/// received and sends every message Renoa had not confirmed.
pub async fn session(
    endpoints: Endpoints,
    credentials: DeviceCredentials,
    key: Arc<MacKey>,
    notice_key: Arc<NoticeSecret>,
    pace: Pace,
    mut commands: mpsc::UnboundedReceiver<Command>,
    updates: mpsc::UnboundedSender<Update>,
) {
    let mut kept = Kept::default();
    let notices = Notices::new(credentials.device_id, notice_key);
    let mut connection_id = 0_u64;
    loop {
        connection_id = connection_id.wrapping_add(1);
        let connected = within(connect_with_key(
            &endpoints.connect,
            credentials.clone(),
            key.as_ref(),
        ))
        .await;
        let (connection, events) = match connected {
            Ok(linked) => linked,
            Err(ClientError::Rejected {
                code: ErrorCode::AuthenticationFailed,
                ..
            }) => {
                let _ = updates.send(Update::SignedOut);
                return;
            }
            Err(_) => {
                let _ = updates.send(Update::Linked(false));
                if offline(pace.retry_after, &mut commands, &mut kept).await == Ended::ViewsGone {
                    return;
                }
                continue;
            }
        };
        let refresh_notices = Arc::new(Notify::new());
        // Events are read apart from the calls below, so a long replay never
        // holds up a reply.
        let forwarder = tokio::spawn(forward(
            events,
            Arc::clone(&kept.through),
            updates.clone(),
            Arc::clone(&refresh_notices),
        ));
        let _ = updates.send(Update::Linked(true));
        let ended = linked(
            connection_id,
            &connection,
            &endpoints.origin,
            pace.agents_every,
            &notices,
            &refresh_notices,
            &mut commands,
            &updates,
            &mut kept,
        )
        .await;
        forwarder.abort();
        drop(connection);
        if ended == Ended::ViewsGone {
            return;
        }
        let _ = updates.send(Update::Linked(false));
        if offline(pace.retry_after, &mut commands, &mut kept).await == Ended::ViewsGone {
            return;
        }
    }
}

/// Waits `wait` before trying Renoa again, keeping what the views ask
/// meanwhile for the next connection. A Host call is answered Lost at once:
/// it is not made later, when the owner may have given up on it.
async fn offline(
    wait: Duration,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    kept: &mut Kept,
) -> Ended {
    let waited = sleep(wait);
    tokio::pin!(waited);
    loop {
        tokio::select! {
            () = &mut waited => return Ended::LinkLost,
            command = commands.recv() => match command {
                None => return Ended::ViewsGone,
                Some(command) => match kept.hold(command) {
                    None => {}
                    Some(Immediate::Host(call)) => {
                        let _ = call.answered.send(host::Answer::Lost);
                    }
                    Some(Immediate::ShareLink { answered, .. }) => {
                        let _ = answered.send(None);
                    }
                    Some(Immediate::Stop { answered, .. }) => {
                        let _ = answered.send(Err(StopProblem::Uncertain));
                    }
                    Some(Immediate::AcknowledgeNotice { answered, .. }) => {
                        let _ = answered.send(false);
                    }
                },
            },
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Ended {
    LinkLost,
    ViewsGone,
}

/// Serves the views over one connection until it is lost or they are gone.
#[expect(clippy::too_many_arguments)]
async fn linked(
    connection_id: u64,
    connection: &Connection,
    origin: &str,
    agents_every: Duration,
    notices: &Notices,
    refresh_notices: &Arc<Notify>,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    updates: &mpsc::UnboundedSender<Update>,
    kept: &mut Kept,
) -> Ended {
    for chat in kept.followed.values_mut() {
        chat.attached = false;
    }
    // The first tick is at once: it lists the agents and catches up.
    let mut agents = tokio::time::interval(agents_every);
    agents.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut first_tick = true;
    // Profiles and setup notices are read beside the loop, coalesced to one
    // in-flight reader each so slow replies never stack or hold up messages.
    let mut reading: Option<Aborting> = None;
    let mut reading_notices: Option<Aborting> = Some(Aborting(tokio::spawn(notices.clone().sync(
        connection_id,
        connection.clone(),
        updates.clone(),
    ))));
    let mut calls: Vec<Aborting> = Vec::new();
    let trigger_notices = |slot: &mut Option<Aborting>| {
        if slot.as_ref().is_none_or(Aborting::finished) {
            *slot = Some(Aborting(tokio::spawn(notices.clone().sync(
                connection_id,
                connection.clone(),
                updates.clone(),
            ))));
        } else {
            notices.mark_dirty();
        }
    };
    loop {
        let served = tokio::select! {
            command = commands.recv() => match command {
                None => return Ended::ViewsGone,
                Some(command) => match kept.hold(command) {
                    None => catch_up(connection, updates, kept).await,
                    Some(Immediate::Host(call)) => {
                        let connection = connection.clone();
                        // A call the link outlives is answered Lost when it ends.
                        calls.push(Aborting(tokio::spawn(async move {
                            let answer = host::ask(&connection, call.method, &call.path, call.body).await;
                            let _ = call.answered.send(answer);
                        })));
                        calls.retain(|call| !call.finished());
                        Ok(())
                    }
                    Some(Immediate::ShareLink { share_id, answered }) => {
                        let connection = connection.clone();
                        let origin = origin.to_owned();
                        calls.push(Aborting(tokio::spawn(async move {
                            let url = within(connection.share_link(share_id))
                                .await
                                .ok()
                                .filter(|path| path.expose().starts_with("/s/"))
                                .map(|path| format!("{origin}{}", path.expose()));
                            let _ = answered.send(url);
                        })));
                        calls.retain(|call| !call.finished());
                        Ok(())
                    }
                    Some(Immediate::Stop {
                        task_id,
                        command_id,
                        answered,
                    }) => {
                        let connection = connection.clone();
                        calls.push(Aborting(tokio::spawn(request_stop(
                            connection, task_id, command_id, answered,
                        ))));
                        calls.retain(|call| !call.finished());
                        Ok(())
                    }
                    Some(Immediate::AcknowledgeNotice {
                        notice_id,
                        answered,
                    }) => {
                        calls.push(Aborting(tokio::spawn(notices.clone().acknowledge(
                            connection.clone(),
                            notice_id,
                            updates.clone(),
                            answered,
                        ))));
                        calls.retain(|call| !call.finished());
                        Ok(())
                    }
                },
            },
            () = refresh_notices.notified() => {
                trigger_notices(&mut reading_notices);
                Ok(())
            }
            _ = agents.tick() => match within(connection.list_targets()).await {
                Ok(listed) => {
                    if reading.as_ref().is_none_or(Aborting::finished) {
                        reading = Some(Aborting(tokio::spawn(profiles::read(
                            connection.clone(),
                            listed.clone(),
                            Arc::clone(&kept.profiles),
                            updates.clone(),
                        ))));
                    }
                    if !std::mem::replace(&mut first_tick, false) {
                        trigger_notices(&mut reading_notices);
                    }
                    let _ = updates.send(Update::Agents(listed));
                    catch_up(connection, updates, kept).await
                }
                Err(error) => Err(error),
            },
            () = connection.closed() => return Ended::LinkLost,
        };
        if served.is_err() {
            return Ended::LinkLost;
        }
    }
}

/// Follows every chat not yet followed on this connection, then sends every
/// waiting message.
async fn catch_up(
    connection: &Connection,
    updates: &mpsc::UnboundedSender<Update>,
    kept: &mut Kept,
) -> Result<(), ClientError> {
    for (task_id, chat) in kept.followed.iter_mut().filter(|(_, chat)| !chat.attached) {
        follow(connection, *task_id, chat, &kept.through).await?;
    }
    flush(connection, &mut kept.outbox, updates).await
}

pub mod host;
mod notices;
mod profiles;
mod session;
mod sign_in;

pub use profiles::host_agent;
#[cfg(test)]
mod session_tests;
#[cfg(test)]
mod tests;

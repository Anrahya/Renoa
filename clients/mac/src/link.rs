//! The app's link to Renoa, run on a tokio runtime beside GPUI's own
//! executor. Views send it commands and read its updates over channels.

use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use renoa_protocol::{CommandId, CommandInput, TargetRef};
use renoa_rcp_client::{ClientError, Connection, Events, connect_with_key};
use renoa_rcp_wire::{DeviceCredentials, ErrorCode, NodeId, TargetSummary, TaskEvent, TaskId};
use tokio::{
    runtime::Runtime,
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::{MissedTickBehavior, sleep},
};
use uuid::Uuid;

use crate::{face::Look, key::MacKey};
use host::HostCall;
pub use sign_in::{SignInStep, sign_in};

const DEFAULT_ORIGIN: &str = "https://renoa.live";
/// How long a call may wait for Renoa before the link counts as lost, as the
/// Android app waits (`Timeouts.reply`).
const REPLY_WITHIN: Duration = Duration::from_secs(30);

pub fn runtime() -> &'static Runtime {
    static RUNTIME: LazyLock<Runtime> =
        LazyLock::new(|| Runtime::new().expect("start the tokio runtime"));
    &RUNTIME
}

/// Where Renoa is: `RENOA_ORIGIN`, or renoa.live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub origin: String,
    pub connect: String,
}

impl Endpoints {
    pub fn from_environment() -> Self {
        Self::at(&std::env::var("RENOA_ORIGIN").unwrap_or_else(|_| DEFAULT_ORIGIN.to_owned()))
    }

    /// The coordinator at `origin` takes connections at `/connect`, over
    /// TLS unless the origin is plain HTTP.
    pub fn at(origin: &str) -> Self {
        let origin = origin.trim_end_matches('/').to_owned();
        let socket = origin
            .strip_prefix("https://")
            .map(|host| format!("wss://{host}"))
            .or_else(|| {
                origin
                    .strip_prefix("http://")
                    .map(|host| format!("ws://{host}"))
            })
            .unwrap_or_else(|| origin.clone());
        Self {
            connect: format!("{socket}/connect"),
            origin,
        }
    }
}

/// How often signing in collects, how long the link waits before trying
/// Renoa again after a failure, and how often it reads the agent list again.
#[derive(Debug, Clone, Copy)]
pub struct Pace {
    pub collect_every: Duration,
    pub retry_after: Duration,
    /// Each read shows agents that came online and retries the chats,
    /// messages and profiles a refusal held back. A connection that died
    /// without closing fails it within [`REPLY_WITHIN`].
    pub agents_every: Duration,
}

impl Pace {
    pub const APP: Self = Self {
        collect_every: Duration::from_secs(2),
        retry_after: Duration::from_secs(5),
        agents_every: Duration::from_secs(30),
    };
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
    /// Renoa refused a message to this chat; it was not sent.
    NotSent {
        task_id: TaskId,
        text: String,
    },
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
        /// Told whether Renoa took the message (`true`) or refused it; it
        /// is kept across reconnects until one or the other.
        sent: Option<oneshot::Sender<bool>>,
    },
    /// Calls the Host, beside the chats, and answers on the call.
    Host(HostCall),
}

struct Followed {
    node_id: NodeId,
    target: TargetRef,
    attached: bool,
}

/// A message on its way. Its id stays the same across retries, so Renoa
/// admits it once.
struct Outgoing {
    task_id: TaskId,
    command_id: CommandId,
    text: String,
    sent: Option<oneshot::Sender<bool>>,
}

/// The last event received of each chat, noted as events arrive.
type Through = Arc<Mutex<HashMap<TaskId, u64>>>;

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
    /// a Host call is the caller's to make.
    fn hold(&mut self, command: Command) -> Option<HostCall> {
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
                sent,
            } => {
                self.outbox.push_back(Outgoing {
                    task_id,
                    command_id: CommandId::new(),
                    text,
                    sent,
                });
                None
            }
            Command::Host(call) => Some(call),
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
    pace: Pace,
    mut commands: mpsc::UnboundedReceiver<Command>,
    updates: mpsc::UnboundedSender<Update>,
) {
    let mut kept = Kept::default();
    loop {
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
        // Events are read apart from the calls below, so a long replay never
        // holds up a reply.
        let forwarder = tokio::spawn(forward(events, Arc::clone(&kept.through), updates.clone()));
        let _ = updates.send(Update::Linked(true));
        let ended = linked(
            &connection,
            pace.agents_every,
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
                Some(command) => {
                    if let Some(call) = kept.hold(command) {
                        let _ = call.answered.send(host::Answer::Lost);
                    }
                }
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
async fn linked(
    connection: &Connection,
    agents_every: Duration,
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
    // Profiles are read beside the loop: a slow Host holds up no message.
    let mut reading: Option<Aborting> = None;
    let mut calls: Vec<Aborting> = Vec::new();
    loop {
        let served = tokio::select! {
            command = commands.recv() => match command {
                None => return Ended::ViewsGone,
                Some(command) => match kept.hold(command) {
                    None => catch_up(connection, updates, kept).await,
                    Some(call) => {
                        let connection = connection.clone();
                        // A call the link outlives is answered Lost when it ends.
                        calls.push(Aborting(tokio::spawn(async move {
                            let answer = host::ask(&connection, call.method, &call.path, call.body).await;
                            let _ = call.answered.send(answer);
                        })));
                        calls.retain(|call| !call.finished());
                        Ok(())
                    }
                },
            },
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

/// A task that ends when the link it works for does.
struct Aborting(JoinHandle<()>);

impl Aborting {
    fn finished(&self) -> bool {
        self.0.is_finished()
    }
}

impl Drop for Aborting {
    fn drop(&mut self) {
        self.0.abort();
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

/// Passes events on as they arrive and notes the last one of each chat.
async fn forward(mut events: Events, through: Through, updates: mpsc::UnboundedSender<Update>) {
    while let Some(Ok(event)) = events.next().await {
        note(&through, event.task_id, event.sequence);
        if updates.send(Update::Event(Box::new(event))).is_err() {
            return;
        }
    }
}

/// Notes that `task_id` was received through `sequence`; it never moves back.
fn note(through: &Through, task_id: TaskId, sequence: u64) {
    let mut through = through.lock().expect("through lock");
    let last = through.entry(task_id).or_insert(sequence);
    *last = (*last).max(sequence);
}

/// Opens a chat's task, an exact retry when another app opened it first, and
/// attaches after the last event received. A refusal leaves the chat for the
/// next tick; only a lost link is an error.
async fn follow(
    connection: &Connection,
    task_id: TaskId,
    chat: &mut Followed,
    through: &Through,
) -> Result<(), ClientError> {
    let opened = within(connection.open_task(task_id, chat.node_id, chat.target.clone())).await;
    if !settled(connection, opened).await? {
        return Ok(());
    }
    let after = through.lock().expect("through lock").get(&task_id).copied();
    chat.attached = settled(connection, within(connection.attach(task_id, after)).await).await?;
    Ok(())
}

/// Sends every waiting message in order. A refusal drops a message and tells
/// its sender, or else the views; a lost link keeps it, and the rest, for the next connection.
async fn flush(
    connection: &Connection,
    outbox: &mut VecDeque<Outgoing>,
    updates: &mpsc::UnboundedSender<Update>,
) -> Result<(), ClientError> {
    while let Some(next) = outbox.front() {
        let input = CommandInput::from_text(next.text.clone());
        let submitted = within(connection.submit(next.task_id, next.command_id, input)).await;
        let took = settled(connection, submitted).await?;
        let Some(done) = outbox.pop_front() else {
            break;
        };
        match done.sent {
            // Whoever sent it hears how it went, and shows a refusal itself.
            Some(sent) => {
                let _ = sent.send(took);
            }
            None if !took => {
                let _ = updates.send(Update::NotSent {
                    task_id: done.task_id,
                    text: done.text,
                });
            }
            None => {}
        }
    }
    Ok(())
}

/// Whether a call succeeded, when Renoa answered it: a refusal is `false`.
/// A failure that ended the connection, whatever its kind, is the error:
/// the call may have been admitted before it ended.
async fn settled<T>(
    connection: &Connection,
    call: Result<T, ClientError>,
) -> Result<bool, ClientError> {
    match call {
        Ok(_) => Ok(true),
        Err(error @ ClientError::Transport(_)) => Err(error),
        Err(error) if ended(connection).await => Err(error),
        Err(_) => Ok(false),
    }
}

/// Whether the connection has ended. It ends before it fails its waiting
/// calls, so a call failed by the end sees it ended.
async fn ended(connection: &Connection) -> bool {
    tokio::select! {
        biased;
        () = connection.closed() => true,
        () = std::future::ready(()) => false,
    }
}

/// `call`, or a transport failure once Renoa has not answered in
/// [`REPLY_WITHIN`].
async fn within<T>(call: impl Future<Output = Result<T, ClientError>>) -> Result<T, ClientError> {
    tokio::time::timeout(REPLY_WITHIN, call)
        .await
        .unwrap_or_else(|_| Err(ClientError::Transport("Renoa did not answer".to_owned())))
}

pub mod host;
mod profiles;
mod sign_in;

pub use profiles::host_agent;
#[cfg(test)]
mod session_tests;
#[cfg(test)]
mod tests;

//! The app's link to Renoa, run on a tokio runtime beside GPUI's own
//! executor. Views send it commands and read its updates over channels.

use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use renoa_protocol::{CommandId, CommandInput, TargetRef};
use renoa_rcp_client::{
    ClientError, Collected, Connection, Events, ask_to_sign_in, collect_sign_in, connect_with_key,
};
use renoa_rcp_wire::{
    DeviceCode, DeviceCredentials, ErrorCode, NodeId, TargetSummary, TaskEvent, TaskId,
};
use tokio::{runtime::Runtime, sync::mpsc, time::sleep};
use uuid::Uuid;

use crate::key::MacKey;

const DEFAULT_ORIGIN: &str = "https://renoa.live";
/// How long a call may wait for Renoa before the link counts as lost, as the
/// Android app waits (`Timeouts.reply`).
const REPLY_WITHIN: Duration = Duration::from_secs(30);
/// How often the agent list is read again, to show agents that came online
/// and to retry chats and messages a refusal held back. A connection that
/// died without closing fails this read within [`REPLY_WITHIN`].
const AGENTS_EVERY: Duration = Duration::from_secs(30);

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

/// How often signing in collects, and how long the link waits before trying
/// Renoa again after a failure.
#[derive(Debug, Clone, Copy)]
pub struct Pace {
    pub collect_every: Duration,
    pub retry_after: Duration,
}

impl Pace {
    pub const APP: Self = Self {
        collect_every: Duration::from_secs(2),
        retry_after: Duration::from_secs(5),
    };
}

/// What signing in shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInStep {
    /// The code to type on the phone.
    Showing(DeviceCode),
    /// Whether Renoa answered the last call.
    Reachable(bool),
    /// The credential is kept; the Mac is signed in.
    SignedIn(DeviceCredentials),
    /// The approved credential could not be kept.
    NotKept(String),
}

/// Asks for a sign-in and waits for the owner's phone to approve it. Only an
/// expired request is replaced by a new one with a new code; any other
/// failure asks again under the same id after a pause, which converges on
/// one request. The credential is kept with `keep` before the views hear of
/// it, so an approval is kept even when no view is left to show it. Stops
/// once no view listens.
pub async fn sign_in(
    endpoints: Endpoints,
    key: Arc<MacKey>,
    label: String,
    pace: Pace,
    keep: impl Fn(&DeviceCredentials) -> Result<(), String>,
    steps: mpsc::UnboundedSender<SignInStep>,
) {
    let mut reachable = true;
    let mut heard = |error: Option<&ClientError>| {
        let answered = !matches!(error, Some(ClientError::Transport(_)));
        if answered != reachable {
            reachable = answered;
            let _ = steps.send(SignInStep::Reachable(answered));
        }
    };
    loop {
        let request_id = Uuid::new_v4();
        let asked = loop {
            let asked = ask_to_sign_in(&endpoints.origin, request_id, "mac", &label, key.as_ref());
            match within(asked).await {
                Ok(asked) => break asked,
                Err(error) => heard(Some(&error)),
            }
            if steps.is_closed() {
                return;
            }
            sleep(pace.retry_after).await;
        };
        heard(None);
        if steps.send(SignInStep::Showing(asked.code)).is_err() {
            return;
        }
        loop {
            sleep(pace.collect_every).await;
            match within(collect_sign_in(&endpoints.origin, request_id, key.as_ref())).await {
                Ok(Collected::Waiting { .. }) => heard(None),
                Ok(Collected::SignedIn(credentials)) => {
                    let step = match keep(&credentials) {
                        Ok(()) => SignInStep::SignedIn(credentials),
                        Err(problem) => SignInStep::NotKept(problem),
                    };
                    let _ = steps.send(step);
                    return;
                }
                // The request expired: show a new code.
                Err(ClientError::Rejected {
                    code: ErrorCode::NotFound,
                    ..
                }) => break,
                Err(error) => {
                    heard(Some(&error));
                    sleep(pace.retry_after).await;
                }
            }
            if steps.is_closed() {
                return;
            }
        }
    }
}

/// What the session tells the views.
#[derive(Debug, Clone)]
pub enum Update {
    Linked(bool),
    Agents(Vec<TargetSummary>),
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
    },
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
}

/// The last event received of each chat, noted as events arrive.
type Through = Arc<Mutex<HashMap<TaskId, u64>>>;

/// What the session keeps across connections.
#[derive(Default)]
struct Kept {
    through: Through,
    followed: HashMap<TaskId, Followed>,
    outbox: VecDeque<Outgoing>,
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
                sleep(pace.retry_after).await;
                continue;
            }
        };
        // Events are read apart from the calls below, so a long replay never
        // holds up a reply.
        let forwarder = tokio::spawn(forward(events, Arc::clone(&kept.through), updates.clone()));
        let _ = updates.send(Update::Linked(true));
        let ended = linked(&connection, &mut commands, &updates, &mut kept).await;
        forwarder.abort();
        drop(connection);
        if ended == Ended::ViewsGone {
            return;
        }
        let _ = updates.send(Update::Linked(false));
        sleep(pace.retry_after).await;
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
    commands: &mut mpsc::UnboundedReceiver<Command>,
    updates: &mpsc::UnboundedSender<Update>,
    kept: &mut Kept,
) -> Ended {
    for chat in kept.followed.values_mut() {
        chat.attached = false;
    }
    // The first tick is at once: it lists the agents and catches up.
    let mut agents = tokio::time::interval(AGENTS_EVERY);
    loop {
        let served = tokio::select! {
            command = commands.recv() => match command {
                None => return Ended::ViewsGone,
                Some(Command::Follow { task_id, node_id, target, after }) => {
                    if let Some(after) = after {
                        note(&kept.through, task_id, after);
                    }
                    kept.followed
                        .entry(task_id)
                        .or_insert(Followed { node_id, target, attached: false });
                    catch_up(connection, updates, kept).await
                }
                Some(Command::Send { task_id, text }) => {
                    kept.outbox.push_back(Outgoing { task_id, command_id: CommandId::new(), text });
                    flush(connection, &mut kept.outbox, updates).await
                }
            },
            _ = agents.tick() => match within(connection.list_targets()).await {
                Ok(listed) => {
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
/// the views; a lost link keeps it, and the rest, for the next connection.
async fn flush(
    connection: &Connection,
    outbox: &mut VecDeque<Outgoing>,
    updates: &mpsc::UnboundedSender<Update>,
) -> Result<(), ClientError> {
    while let Some(next) = outbox.front() {
        let input = CommandInput::from_text(next.text.clone());
        let submitted = within(connection.submit(next.task_id, next.command_id, input)).await;
        if !settled(connection, submitted).await? {
            let _ = updates.send(Update::NotSent {
                task_id: next.task_id,
                text: next.text.clone(),
            });
        }
        outbox.pop_front();
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

#[cfg(test)]
mod session_tests;
#[cfg(test)]
mod tests;

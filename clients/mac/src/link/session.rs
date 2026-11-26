//! Reconnectable link-session mechanics: tracking received task cursors,
//! opening/attaching followed chats, and flushing queued outbound messages.

use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

use renoa_protocol::{Author, CommandId, CommandInput, ExecutionEventKind, TargetRef};
use renoa_rcp_client::{ClientError, Connection, Events, StopOutcome, SurfaceEvent};
use renoa_rcp_wire::{NodeId, TaskEventKind, TaskId};
use tokio::{
    sync::{Notify, mpsc, oneshot},
    task::JoinHandle,
};

use super::Update;

const DEFAULT_ORIGIN: &str = "https://renoa.live";

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

/// Why stopping an active command did not confirm a stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopProblem {
    /// Coordinator explicitly refused the stop request.
    Refused,
    /// The stop may or may not have landed (offline, timeout, or link dropped).
    Uncertain,
}

/// Sends one stop request on the current connection and maps any refusal or
/// lost-link outcome for the requesting view.
pub(super) async fn request_stop(
    connection: Connection,
    task_id: TaskId,
    command_id: CommandId,
    answered: oneshot::Sender<Result<StopOutcome, StopProblem>>,
) {
    let reply = match within(connection.stop_command(task_id, command_id)).await {
        Ok(outcome) => Ok(outcome),
        Err(ClientError::Rejected { .. }) => Err(StopProblem::Refused),
        Err(_) => Err(StopProblem::Uncertain),
    };
    let _ = answered.send(reply);
}

/// How long a call may wait for Renoa before the link counts as lost, as the
/// Android app waits (`Timeouts.reply`).
const REPLY_WITHIN: Duration = Duration::from_secs(30);

pub(super) struct Followed {
    pub(super) node_id: NodeId,
    pub(super) target: TargetRef,
    pub(super) attached: bool,
}

/// A message on its way. Its id stays the same across retries, so Renoa
/// admits it once.
pub(super) struct Outgoing {
    pub(super) task_id: TaskId,
    pub(super) command_id: CommandId,
    pub(super) text: String,
    pub(super) steer: bool,
    pub(super) sent: Option<oneshot::Sender<bool>>,
}

/// The last event received of each chat, noted as events arrive.
pub(super) type Through = Arc<Mutex<HashMap<TaskId, u64>>>;

/// Notes that `task_id` was received through `sequence`; it never moves back.
pub(super) fn note(through: &Through, task_id: TaskId, sequence: u64) {
    let mut through = through.lock().expect("through lock");
    let last = through.entry(task_id).or_insert(sequence);
    *last = (*last).max(sequence);
}

/// Opens a chat's task, an exact retry when another app opened it first, and
/// attaches after the last event received. A refusal leaves the chat for the
/// next tick; only a lost link is an error.
pub(super) async fn follow(
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
    chat.attached = settled(
        connection,
        within(connection.attach_live(task_id, after)).await,
    )
    .await?;
    Ok(())
}

/// Sends every waiting message in order. A refusal drops a message and tells
/// its sender, or else the views; a lost link keeps it, and the rest, for the next connection.
pub(super) async fn flush(
    connection: &Connection,
    outbox: &mut VecDeque<Outgoing>,
    updates: &mpsc::UnboundedSender<Update>,
) -> Result<(), ClientError> {
    while let Some(next) = outbox.front() {
        let input = CommandInput::Text {
            text: next.text.clone(),
            context: None,
            author: Author::Principal,
            quiet: false,
            steer: next.steer,
            attachments: Vec::new(),
        };
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
pub(super) async fn settled<T>(
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
pub(super) async fn within<T>(
    call: impl Future<Output = Result<T, ClientError>>,
) -> Result<T, ClientError> {
    tokio::time::timeout(REPLY_WITHIN, call)
        .await
        .unwrap_or_else(|_| Err(ClientError::Transport("Renoa did not answer".to_owned())))
}

/// A task that ends when the link it works for does.
pub(super) struct Aborting(pub(super) JoinHandle<()>);

impl Aborting {
    pub(super) fn finished(&self) -> bool {
        self.0.is_finished()
    }
}

impl Drop for Aborting {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Passes events on as they arrive, notes the last one of each chat, and wakes
/// notice reads whenever an agent turn ends (as a setup link ends its turn).
pub(super) async fn forward(
    mut events: Events,
    through: Through,
    updates: mpsc::UnboundedSender<Update>,
    refresh_notices: Arc<Notify>,
) {
    while let Some(Ok(item)) = events.next_event().await {
        let send = match item {
            SurfaceEvent::Task(event) => {
                note(&through, event.task_id, event.sequence);
                if let TaskEventKind::ExecutionEvent { event: exec, .. } = &event.kind
                    && matches!(exec.kind, ExecutionEventKind::ExecutionTerminated { .. })
                {
                    refresh_notices.notify_one();
                }
                Update::Event(event)
            }
            SurfaceEvent::Live {
                task_id,
                command_id,
                activity,
            } => Update::Live {
                task_id,
                command_id,
                activity,
            },
        };
        if updates.send(send).is_err() {
            return;
        }
    }
}

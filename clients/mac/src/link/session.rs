//! Reconnectable link-session mechanics: tracking received task cursors,
//! opening/attaching followed chats, and flushing queued outbound messages.

use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

use renoa_protocol::{CommandId, CommandInput, TargetRef};
use renoa_rcp_client::{ClientError, Connection};
use renoa_rcp_wire::{NodeId, TaskId};
use tokio::sync::{mpsc, oneshot};

use super::Update;

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

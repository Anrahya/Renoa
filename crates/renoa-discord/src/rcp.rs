//! Discord's link to the Host's agents through the RCP coordinator.
//!
//! Queued Discord messages become commands on their channel's task; task
//! records come back in order and become reply pages. Every step is durable in
//! the surface store first, so a reconnect resumes from the stored cursors and
//! resubmits unconfirmed commands under their original identities.

use std::{sync::Arc, time::Duration};

use renoa_control::{DeviceCredentials, ErrorCode, TaskId};
use renoa_protocol::{CommandId, TargetRef};
use renoa_rcp_client::{ClientError, Connection, Events};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::{
    DiscordError,
    progress::{Progress, Step},
    store::{Applied, QueuedTurn, SurfaceStore},
};

const EMPTY_PROMPT: &str = "Send a task after the mention.";
const OFFLINE: &str =
    "This agent's machine is offline, so the message was not sent. Send it again once it is back.";
const MAX_BACKOFF: Duration = Duration::from_secs(30);

pub(crate) struct Link {
    pub(crate) endpoint: String,
    pub(crate) credentials: DeviceCredentials,
    pub(crate) store: Arc<SurfaceStore>,
    /// Signalled when the gateway queues a message.
    pub(crate) turns: Arc<Notify>,
    /// Signalled when reply pages are ready to post.
    pub(crate) deliveries: Arc<Notify>,
    /// Receives each applied record's step for transient progress.
    pub(crate) progress: Progress,
}

/// Keeps the coordinator link open until shutdown, reconnecting with bounded
/// backoff. Only a refused credential stops the surface.
pub(crate) async fn maintain(link: Link, shutdown: CancellationToken) -> Result<(), DiscordError> {
    let mut backoff = Duration::from_secs(1);
    loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }
        let ended = match renoa_rcp_client::connect(&link.endpoint, link.credentials.clone()).await
        {
            Ok((connection, events)) => {
                log("info", "rcp_connected", &serde_json::json!({}));
                backoff = Duration::from_secs(1);
                serve(&link, &connection, events, &shutdown).await
            }
            Err(error) => Err(error.into()),
        };
        match ended {
            Ok(()) => return Ok(()),
            Err(DiscordError::Rcp(error))
                if error.code() == Some(ErrorCode::AuthenticationFailed) =>
            {
                return Err(DiscordError::Rcp(error));
            }
            Err(error) => log(
                "warn",
                "rcp_disconnected",
                &serde_json::json!({ "error": error.to_string(), "retry_ms": backoff.as_millis() }),
            ),
        }
        tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            () = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

async fn serve(
    link: &Link,
    connection: &Connection,
    mut events: Events,
    shutdown: &CancellationToken,
) -> Result<(), DiscordError> {
    for (task_id, cursor) in link.store.opened_tasks()? {
        connection
            .attach(TaskId::from_uuid(task_id), cursor)
            .await?;
    }
    loop {
        submit_queued(link, connection).await?;
        tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            () = link.turns.notified() => {}
            event = events.next() => match event {
                Some(Ok(event)) => {
                    let applied = link.store.apply_event(&event)?;
                    if applied == Applied::ReplyReady {
                        link.deliveries.notify_one();
                    }
                    if applied != Applied::Stale {
                        let (command_id, step) = Step::of(&event.kind);
                        if let Some(target) = link.store.progress_target(&command_id)? {
                            link.progress.observe(command_id, target, step);
                        }
                    }
                }
                Some(Err(error)) => return Err(error.into()),
                None => return Err(ClientError::Transport("the event stream ended".to_owned()).into()),
            }
        }
    }
}

/// Submits every queued message in Discord order. A transport failure leaves
/// the message queued for an exact retry after reconnecting.
async fn submit_queued(link: &Link, connection: &Connection) -> Result<(), DiscordError> {
    while let Some(turn) = link.store.next_queued()? {
        if turn.prompt.is_empty() {
            answer(link, &turn, EMPTY_PROMPT)?;
            continue;
        }
        if !turn.opened && !open(link, connection, &turn).await? {
            continue;
        }
        let task_id = TaskId::from_uuid(turn.task_id);
        let command_id = CommandId::from_uuid(turn.command_id);
        match connection
            .submit(task_id, command_id, turn.prompt.clone())
            .await
        {
            Ok(()) => {
                link.store.mark_submitted(&turn.message_id)?;
                log(
                    "info",
                    "message_submitted",
                    &serde_json::json!({
                        "discord_message_id": turn.message_id,
                        "task_id": task_id,
                        "command_id": command_id,
                    }),
                );
            }
            Err(error) => refuse(link, &turn, error)?,
        }
    }
    Ok(())
}

/// Opens the turn's task on the node advertising its agent. Returns whether
/// the task is open; a turn whose agent is unreachable is answered instead.
async fn open(
    link: &Link,
    connection: &Connection,
    turn: &QueuedTurn,
) -> Result<bool, DiscordError> {
    let target = TargetRef::new(format!("agent:{}", turn.agent_id));
    let node = connection
        .list_targets()
        .await?
        .into_iter()
        .find(|summary| summary.target == target)
        .map(|summary| summary.node_id);
    let Some(node_id) = node else {
        answer(link, turn, OFFLINE)?;
        return Ok(false);
    };
    let task_id = TaskId::from_uuid(turn.task_id);
    match connection.open_task(task_id, node_id, target).await {
        Ok(()) => {}
        Err(error) => {
            refuse(link, turn, error)?;
            return Ok(false);
        }
    }
    link.store.mark_opened(turn.task_id)?;
    connection.attach(task_id, None).await?;
    log(
        "info",
        "task_opened",
        &serde_json::json!({
            "task_id": task_id,
            "node_id": node_id,
            "agent_id": turn.agent_id,
        }),
    );
    Ok(true)
}

fn refuse(link: &Link, turn: &QueuedTurn, error: ClientError) -> Result<(), DiscordError> {
    let text = match error.code() {
        None => return Err(error.into()),
        Some(ErrorCode::NodeOffline | ErrorCode::NotFound) => OFFLINE.to_owned(),
        Some(_) => format!("Renoa could not accept this message: {error}"),
    };
    log(
        "warn",
        "message_refused",
        &serde_json::json!({
            "discord_message_id": turn.message_id,
            "task_id": turn.task_id,
            "error": error.to_string(),
        }),
    );
    answer(link, turn, &text)
}

fn answer(link: &Link, turn: &QueuedTurn, text: &str) -> Result<(), DiscordError> {
    link.store.answer_locally(&turn.message_id, text)?;
    link.deliveries.notify_one();
    Ok(())
}

fn log(level: &'static str, name: &'static str, fields: &serde_json::Value) {
    renoa_telemetry::event("renoa.discord", level, name, fields);
}

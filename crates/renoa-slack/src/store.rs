use std::{
    fs::File,
    path::Path,
    sync::{Arc, Mutex},
};

use rusqlite::Connection;
use uuid::Uuid;

use crate::{SlackError, commands::Command, ingress::Topic};

mod actions;
mod admission;
mod schema;
#[cfg(test)]
mod tests;
mod work;

pub(crate) enum AgentSelection {
    Unchanged,
    Selected(Uuid),
    Rejected(String),
}

#[derive(Clone)]
pub(crate) struct Store(Arc<Storage>);
struct Storage {
    connection: Mutex<Connection>,
    _lease: File,
}

pub(crate) struct Binding<'a> {
    pub(crate) host_id: Uuid,
    pub(crate) agent_id: Uuid,
    pub(crate) team: &'a str,
    pub(crate) bot: &'a str,
    pub(crate) user: &'a str,
    pub(crate) workspace: &'a Path,
}

pub(crate) struct Admission {
    pub(crate) queued: bool,
    pub(crate) cancel_target: Option<Uuid>,
}

pub(crate) struct Work {
    pub(crate) seq: i64,
    pub(crate) topic: Topic,
    pub(crate) session_id: Uuid,
    pub(crate) request_id: Uuid,
    pub(crate) command: Command,
    pub(crate) observed_at_ms: i64,
    pub(crate) surface_context: Option<String>,
    pub(crate) reply_ts: Option<String>,
    pub(crate) reply_pending: bool,
    pub(crate) cancel_target: Option<Uuid>,
}

pub(crate) struct Delivery {
    pub(crate) seq: i64,
    pub(crate) chunk: i64,
    pub(crate) topic: Topic,
    pub(crate) text: String,
    pub(crate) ts: Option<String>,
}

impl Store {
    pub(crate) fn open(directory: &Path, binding: &Binding<'_>) -> Result<Self, SlackError> {
        let (lease, mut connection) = schema::open(directory)?;
        schema::bind(&mut connection, binding)?;
        // A model command is replayed using its original kernel identity. An
        // unreceipted post cannot safely be repeated; a known message update can.
        connection.execute_batch(
            "BEGIN IMMEDIATE;
             UPDATE requests SET state='queued' WHERE state='running';
             UPDATE setup_actions SET state='unknown' WHERE state='sending';
             UPDATE requests SET reply_state='unknown' WHERE reply_state='sending';
             UPDATE deliveries SET state=CASE WHEN slack_ts IS NULL THEN 'unknown' ELSE 'pending' END
               WHERE state='sending';
             COMMIT;"
        )?;
        Ok(Self(Arc::new(Storage {
            connection: Mutex::new(connection),
            _lease: lease,
        })))
    }

    pub(super) async fn run<T: Send + 'static>(
        &self,
        action: impl FnOnce(&mut Connection) -> Result<T, SlackError> + Send + 'static,
    ) -> Result<T, SlackError> {
        let storage = Arc::clone(&self.0);
        tokio::task::spawn_blocking(move || {
            let mut connection = storage
                .connection
                .lock()
                .map_err(|_| SlackError::Invalid("Slack database lock poisoned".to_owned()))?;
            action(&mut connection)
        })
        .await?
    }
}

pub(super) fn uuid(value: &str) -> Result<Uuid, SlackError> {
    Uuid::parse_str(value)
        .map_err(|e| SlackError::Invalid(format!("invalid stored request identity: {e}")))
}

#[derive(Clone, Copy)]
pub(crate) enum ReplyState {
    Pending,
    Sending,
    Known,
    Unknown,
    Failed,
}
impl ReplyState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sending => "sending",
            Self::Known => "known",
            Self::Unknown => "unknown",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum DeliveryState {
    Pending,
    Sending,
    Sent,
    Unknown,
    Failed,
}
impl DeliveryState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sending => "sending",
            Self::Sent => "sent",
            Self::Unknown => "unknown",
            Self::Failed => "failed",
        }
    }
}

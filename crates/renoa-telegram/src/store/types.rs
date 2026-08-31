use thiserror::Error;
use uuid::Uuid;

use crate::ingress::Topic;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StoreError {
    #[error("Telegram surface storage I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Telegram surface database failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Telegram surface storage task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("Telegram surface state is invalid: {0}")]
    Invalid(String),
}

pub(crate) struct RecoveryReport {
    pub(crate) requeued: usize,
    pub(crate) delivery_unknown: usize,
}

pub(crate) struct Admission {
    pub(crate) duplicate: bool,
    pub(crate) queued: bool,
    pub(crate) immediate: Option<ImmediateAction>,
}

pub(crate) enum ImmediateAction {
    Cancel(Topic),
    Stop { topic: Topic, draft_id: i64 },
}

pub(crate) enum PendingAction {
    Execute(WorkItem),
    Deliver(DeliveryItem),
}

pub(crate) struct WorkItem {
    pub(crate) update_id: i64,
    pub(crate) topic: Topic,
    pub(crate) session_id: Uuid,
    pub(crate) request_id: Uuid,
    pub(crate) draft_id: i64,
    pub(crate) observed_at_ms: i64,
    pub(crate) kind: WorkKind,
}

pub(crate) enum WorkKind {
    Prompt(String),
    Compact,
    New,
    Status,
    Cancel,
    Notice(String),
}

pub(crate) struct DeliveryItem {
    pub(crate) update_id: i64,
    pub(crate) topic: Topic,
    pub(crate) text: String,
    pub(crate) cursor: usize,
}

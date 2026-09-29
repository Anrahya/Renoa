use renoa_kernel::AgentId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{LocalHost, LocalHostError};

mod control;
mod receipts;
pub(crate) mod result_tool;
mod results;
mod runs;
pub use control::{AutomationEnablement, HostAutomationControl};
pub use results::AutomationResultSummary;
mod schedule;
mod scheduler;
pub use scheduler::{AutomationScheduler, ScheduledRun};
pub(super) mod store;
#[cfg(test)]
mod tests;
pub(crate) mod tool;

/// Host-owned timing. Daily schedules use named timezones; intervals use elapsed time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AutomationSchedule {
    Once {
        /// Absolute timestamp with an explicit UTC offset or Z.
        at: String,
    },
    Daily {
        hour: i8,
        minute: i8,
        timezone: String,
    },
    Interval {
        hours: u32,
    },
}

/// A standing task for a persistent agent. Results belong to the agent's Host inbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationSpec {
    pub agent_id: AgentId,
    pub name: String,
    pub prompt: String,
    pub schedule: AutomationSchedule,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationRecord {
    pub id: Uuid,
    pub revision: i64,
    pub spec: AutomationSpec,
    pub next_due_ms: i64,
}

/// Revision-checked management shared by tools and other Host clients.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum AutomationMutation {
    Create {
        spec: AutomationSpec,
    },
    Update {
        id: Uuid,
        expected_revision: i64,
        spec: AutomationSpec,
    },
    SetEnabled {
        id: Uuid,
        expected_revision: i64,
        enabled: bool,
    },
    RunNow {
        id: Uuid,
    },
    Delete {
        id: Uuid,
        expected_revision: i64,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationRun {
    pub sequence: i64,
    pub id: Uuid,
    pub automation_id: Uuid,
    pub agent_id: AgentId,
    pub due_ms: i64,
    pub admitted_at_ms: i64,
    pub prompt: String,
    /// The answer of a succeeded run, or why a run failed or was skipped.
    pub output: Option<String>,
    /// How the run ended; `None` while it is unfinished.
    pub status: Option<RunStatus>,
    /// Tool calls that returned an error in a run that executed. `None` while
    /// unfinished, for a skipped run, and for runs recorded before schema 37.
    pub failed_tool_calls: Option<u32>,
    /// When the Host recorded the outcome; `None` before schema 37.
    pub finished_at_ms: Option<i64>,
}

/// How a finished run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// The agent finished the task. Some of its tool calls may have failed.
    Succeeded,
    /// The run could not be sent, or its execution failed or was stopped.
    Failed,
    /// The run was too late to be worth running and never executed.
    Skipped,
}

impl RunStatus {
    /// The stored and serialized name.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        [Self::Succeeded, Self::Failed, Self::Skipped]
            .into_iter()
            .find(|status| status.as_str() == value)
    }
}

/// How an executed run ended, as its executor reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOutcome {
    /// `Succeeded` or `Failed`; only the Host skips a run.
    pub status: RunStatus,
    pub output: String,
    pub failed_tool_calls: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum AutomationError {
    #[error("invalid automation: {0}")]
    Invalid(String),
    #[error("automation was changed; read its current revision before updating")]
    Conflict,
    #[error("automation not found")]
    NotFound,
    #[error("this automation already has an admitted run")]
    Busy,
    #[error("automation operation was cancelled before commit")]
    Cancelled,
    #[error("this caller does not own the configured Host")]
    Forbidden,
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Catalog(#[from] super::catalog::HostCatalogError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Time(#[from] jiff::Error),
}

impl LocalHost {
    /// Applies a management operation once, retaining its exact result for replay.
    /// An agent manages its own automations; another agent's automations need the
    /// actor's stored selection to contain `agent_manage`.
    /// # Errors
    /// Rejects invalid targets, stale revisions, conflicting replay, or storage failures.
    pub async fn manage_automation(
        &self,
        actor: AgentId,
        operation: Uuid,
        mutation: AutomationMutation,
        now_ms: i64,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<AutomationRecord, LocalHostError> {
        self.manage_automation_from(actor, None, operation, mutation, now_ms, cancellation)
            .await
    }

    /// Applies a management operation an agent sent from one Host session. An
    /// automation the agent creates for itself runs in that session's
    /// conversation.
    pub(super) async fn manage_automation_from(
        &self,
        actor: AgentId,
        session: Option<renoa_kernel::SessionId>,
        operation: Uuid,
        mutation: AutomationMutation,
        now_ms: i64,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<AutomationRecord, LocalHostError> {
        let database = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            store::mutate(
                &database,
                receipts::AutomationActor::Agent { id: actor, session },
                operation,
                mutation,
                now_ms,
                &cancellation,
            )
        })
        .await??)
    }

    /// Lists a bounded page of an agent's automations in stable ID order. Another
    /// agent's automations need the actor's stored selection to contain
    /// `agent_manage`.
    /// # Errors
    /// Rejects unauthorized targets and returns catalog and stored-data failures.
    pub async fn list_automations(
        &self,
        actor: AgentId,
        agent: AgentId,
        after: Option<Uuid>,
    ) -> Result<Vec<AutomationRecord>, LocalHostError> {
        let database = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            let db = super::catalog::open_verified(&database)?;
            store::authorize(&db, actor, agent)?;
            store::list(&db, agent, after)
        })
        .await??)
    }

    /// Reads one complete standing task for inspection or revision-checked
    /// editing. Another agent's automations need the actor's stored selection to
    /// contain `agent_manage`.
    /// # Errors
    /// Rejects unauthorized targets and returns an unknown automation or catalog
    /// failures.
    pub async fn automation(
        &self,
        actor: AgentId,
        id: Uuid,
    ) -> Result<AutomationRecord, LocalHostError> {
        let database = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            let db = super::catalog::open_verified(&database)?;
            let record = store::get(&db, id)?;
            store::authorize(&db, actor, record.spec.agent_id)?;
            Ok::<_, AutomationError>(record)
        })
        .await??)
    }

    /// Reads completed results after a surface's durable delivery cursor.
    /// # Errors
    /// Returns catalog and stored-data failures.
    pub async fn completed_automation_runs(
        &self,
        after: i64,
    ) -> Result<Vec<AutomationRun>, LocalHostError> {
        let database = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || runs::completed(&database, after)).await??)
    }
}

pub(super) use store::{SCHEDULER_TABLE, initialize};

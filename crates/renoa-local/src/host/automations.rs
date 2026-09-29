use renoa_kernel::AgentId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{LocalHost, LocalHostError};

mod control;
mod receipts;
pub(crate) mod result_tool;
mod results;
mod runner;
pub use control::{AutomationEnablement, HostAutomationControl};
pub use results::AutomationResultSummary;
mod schedule;
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
    pub session_id: Uuid,
    pub due_ms: i64,
    pub admitted_at_ms: i64,
    pub prompt: String,
    pub output: Option<String>,
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
        let database = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            store::mutate(
                &database,
                receipts::AutomationActor::Agent(actor),
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
        Ok(tokio::task::spawn_blocking(move || store::completed(&database, after)).await??)
    }
}

pub(super) use store::initialize;

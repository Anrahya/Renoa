use renoa_kernel::AgentId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{LocalHost, LocalHostError};

pub(crate) mod result_tool;
mod results;
mod runner;
pub use results::RoutineResultSummary;
mod schedule;
mod store;
#[cfg(test)]
mod tests;
pub(crate) mod tool;

/// Host-owned timing. Daily schedules use named timezones; intervals use elapsed time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoutineSchedule {
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

/// A standing task for a persistent specialist. Results belong to the agent's Host inbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineSpec {
    pub agent_id: AgentId,
    pub name: String,
    pub prompt: String,
    pub schedule: RoutineSchedule,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineRecord {
    pub id: Uuid,
    pub revision: i64,
    pub spec: RoutineSpec,
    pub next_due_ms: i64,
}

/// Revision-checked management shared by tools and other Host clients.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoutineMutation {
    Create {
        spec: RoutineSpec,
    },
    Update {
        id: Uuid,
        expected_revision: i64,
        spec: RoutineSpec,
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
pub struct RoutineRun {
    pub sequence: i64,
    pub id: Uuid,
    pub routine_id: Uuid,
    pub agent_id: AgentId,
    pub session_id: Uuid,
    pub due_ms: i64,
    pub admitted_at_ms: i64,
    pub prompt: String,
    pub output: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RoutineError {
    #[error("invalid routine: {0}")]
    Invalid(String),
    #[error("routine was changed; read its current revision before updating")]
    Conflict,
    #[error("routine not found")]
    NotFound,
    #[error("this routine already has an admitted run")]
    Busy,
    #[error("routine operation was cancelled before commit")]
    Cancelled,
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
    /// Specialists may manage themselves; Arcee may manage any Host specialist.
    /// # Errors
    /// Rejects invalid targets, stale revisions, conflicting replay, or storage failures.
    pub async fn manage_routine(
        &self,
        actor: AgentId,
        operation: Uuid,
        mutation: RoutineMutation,
        now_ms: i64,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<RoutineRecord, LocalHostError> {
        let database = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            store::mutate(&database, actor, operation, mutation, now_ms, &cancellation)
        })
        .await??)
    }

    /// Lists a bounded page of an agent's routines in stable ID order.
    /// # Errors
    /// Returns catalog and stored-data failures.
    pub async fn list_routines(
        &self,
        agent: AgentId,
        after: Option<Uuid>,
    ) -> Result<Vec<RoutineRecord>, LocalHostError> {
        let database = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || store::list(&database, agent, after)).await??)
    }

    /// Reads one complete standing task for inspection or revision-checked editing.
    /// # Errors
    /// Returns an unknown routine or catalog failures.
    pub async fn routine(&self, id: Uuid) -> Result<RoutineRecord, LocalHostError> {
        let database = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            let db = super::catalog::open_verified(&database)?;
            store::get(&db, id)
        })
        .await??)
    }

    /// Reads completed results after a surface's durable delivery cursor.
    /// # Errors
    /// Returns catalog and stored-data failures.
    pub async fn completed_routine_runs(
        &self,
        after: i64,
    ) -> Result<Vec<RoutineRun>, LocalHostError> {
        let database = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || store::completed(&database, after)).await??)
    }
}

pub(super) use store::initialize;

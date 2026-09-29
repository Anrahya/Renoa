use super::{AutomationError, AutomationRun, LocalHost, LocalHostError, RunStatus, runs, store};
use crate::host::catalog;
use renoa_kernel::AgentId;
use rusqlite::{OptionalExtension as _, params};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct AutomationResultSummary {
    pub sequence: i64,
    pub id: Uuid,
    pub automation_id: Uuid,
    pub due_ms: i64,
    pub task_excerpt: String,
    pub status: RunStatus,
    /// `None` for a skipped run and for runs recorded before schema 37.
    pub failed_tool_calls: Option<u32>,
}

impl LocalHost {
    /// Lists up to 20 finished runs with their status, newest first. Pass the last sequence
    /// as `before` for older results. Another agent's results need the
    /// `agent_manage` capability.
    /// # Errors
    /// Rejects unauthorized targets and invalid stored data.
    pub async fn automation_results(
        &self,
        actor: AgentId,
        agent: AgentId,
        before: Option<i64>,
    ) -> Result<Vec<AutomationResultSummary>, LocalHostError> {
        let path = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            let db = catalog::open_verified(&path)?;
            store::authorize(&db, actor, agent)?;
            let mut q = db.prepare("SELECT sequence,id,automation_id,due_ms,substr(prompt,1,240),status,failed_tool_calls FROM host_automation_runs WHERE agent_id=?1 AND output IS NOT NULL AND (?2 IS NULL OR sequence<?2) ORDER BY sequence DESC LIMIT 20")?;
            let records = q.query_map(params![agent.to_string(),before], |r| Ok(AutomationResultSummary {
                sequence:r.get(0)?, id:store::parse(r,1)?, automation_id:store::parse(r,2)?, due_ms:r.get(3)?, task_excerpt:r.get(4)?,
                status:runs::parse_status(&r.get::<_,String>(5)?)?, failed_tool_calls:r.get(6)?,
            }))?.collect::<Result<Vec<_>,_>>()?;
            Ok::<_,AutomationError>(records)
        }).await??)
    }

    /// Reads a retained run, including its exact task and result, across sessions.
    /// # Errors
    /// Rejects unknown runs, unauthorized targets, and corrupt stored data.
    pub async fn automation_result(
        &self,
        actor: AgentId,
        id: Uuid,
    ) -> Result<AutomationRun, LocalHostError> {
        let path = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            let db = catalog::open_verified(&path)?;
            let run = db
                .query_row(
                    &format!(
                        "SELECT {} FROM host_automation_runs WHERE id=?1",
                        runs::RUN_COLUMNS
                    ),
                    [id.to_string()],
                    runs::run,
                )
                .optional()?
                .ok_or(AutomationError::NotFound)?;
            store::authorize(&db, actor, run.agent_id)?;
            Ok::<_, AutomationError>(run)
        })
        .await??)
    }
}

use super::{LocalHost, LocalHostError, RoutineError, RoutineRun, store};
use crate::host::catalog;
use renoa_kernel::AgentId;
use rusqlite::{OptionalExtension as _, params};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct RoutineResultSummary {
    pub sequence: i64,
    pub id: Uuid,
    pub routine_id: Uuid,
    pub due_ms: i64,
    pub task_excerpt: String,
}

impl LocalHost {
    /// Lists up to 20 completed results, newest first. Pass the last sequence
    /// as `before` for older results. Arcee may inspect any specialist.
    /// # Errors
    /// Rejects unauthorized targets and invalid stored data.
    pub async fn routine_results(
        &self,
        actor: AgentId,
        agent: AgentId,
        before: Option<i64>,
    ) -> Result<Vec<RoutineResultSummary>, LocalHostError> {
        let path = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            let db = catalog::open_verified(&path)?;
            store::authorize(&db, actor, agent)?;
            let mut q = db.prepare("SELECT sequence,id,routine_id,due_ms,substr(prompt,1,240) FROM host_routine_runs WHERE agent_id=?1 AND output IS NOT NULL AND (?2 IS NULL OR sequence<?2) ORDER BY sequence DESC LIMIT 20")?;
            let records = q.query_map(params![agent.to_string(),before], |r| Ok(RoutineResultSummary {
                sequence:r.get(0)?, id:store::parse(r,1)?, routine_id:store::parse(r,2)?, due_ms:r.get(3)?, task_excerpt:r.get(4)?,
            }))?.collect::<Result<Vec<_>,_>>()?;
            Ok::<_,RoutineError>(records)
        }).await??)
    }

    /// Reads a retained run, including its exact task and result, across sessions.
    /// # Errors
    /// Rejects unknown runs, unauthorized targets, and corrupt stored data.
    pub async fn routine_result(
        &self,
        actor: AgentId,
        id: Uuid,
    ) -> Result<RoutineRun, LocalHostError> {
        let path = self.config.database.clone();
        Ok(tokio::task::spawn_blocking(move || {
            let db = catalog::open_verified(&path)?;
            let run = db.query_row("SELECT sequence,id,routine_id,agent_id,session_id,due_ms,admitted_at_ms,prompt,output FROM host_routine_runs WHERE id=?1", [id.to_string()], store::run).optional()?.ok_or(RoutineError::NotFound)?;
            store::authorize(&db, actor, run.agent_id)?;
            Ok::<_,RoutineError>(run)
        }).await??)
    }
}

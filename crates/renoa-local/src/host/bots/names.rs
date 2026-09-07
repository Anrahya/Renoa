use super::{BotSummary, LocalHost, LocalHostError};
use crate::host::catalog;
use renoa_kernel::AgentId;
use rusqlite::{OptionalExtension as _, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// A display-name edit. Creation recipes remain immutable for exact replay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenameBot {
    pub id: AgentId,
    pub expected_name: String,
    pub name: String,
}

pub(in crate::host) fn initialize(
    tx: &rusqlite::Transaction<'_>,
) -> Result<(), catalog::HostCatalogError> {
    tx.execute_batch("CREATE TABLE IF NOT EXISTS host_bot_renames(operation_id TEXT PRIMARY KEY,actor_id TEXT NOT NULL REFERENCES host_agents(agent_id),request_json TEXT NOT NULL,result_json TEXT NOT NULL) STRICT;
    UPDATE host_metadata SET schema_version=17 WHERE singleton=1;")?;
    Ok(())
}
impl LocalHost {
    /// Renames a specialist's Host display name without changing its creation recipe.
    /// The expected name prevents stale edits; operation replay returns its first result.
    /// # Errors
    /// Rejects invalid names, unauthorized targets, stale edits, and storage failures.
    pub async fn rename_bot(
        &self,
        actor: AgentId,
        operation: Uuid,
        edit: RenameBot,
        cancellation: CancellationToken,
    ) -> Result<BotSummary, LocalHostError> {
        if edit.name.trim().is_empty() || edit.name.len() > 512 || edit.name.trim() != edit.name {
            return Err(LocalHostError::InvalidRequest(
                "bot name must contain 1–512 bytes without leading/trailing whitespace".to_owned(),
            ));
        }
        let path = self.config.database.clone();
        tokio::task::spawn_blocking(move||{
            let mut db=catalog::open_verified(&path)?;
            let tx=db.transaction_with_behavior(TransactionBehavior::Immediate).map_err(catalog::HostCatalogError::from)?;
            if cancellation.is_cancelled(){return Err(LocalHostError::BotRenameCancelled)}
            let request=serde_json::to_string(&edit)?;
            let receipt:Option<(String,String,String)>=tx.query_row("SELECT actor_id,request_json,result_json FROM host_bot_renames WHERE operation_id=?1",[operation.to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(catalog::HostCatalogError::from)?;
            if let Some((old_actor,old_request,result))=receipt {
                if old_actor!=actor.to_string() || old_request!=request {return Err(LocalHostError::AgentConflict(edit.id))}
                return Ok(serde_json::from_str(&result)?);
            }
            let allowed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM host_agents a JOIN host_bots b ON b.agent_id=?2 WHERE a.agent_id=?1 AND (a.agent_id=b.agent_id OR a.profile_id=?3))",params![actor.to_string(),edit.id.to_string(),crate::ARCEE_PROFILE_ID],|r|r.get(0)).map_err(catalog::HostCatalogError::from)?;
            if !allowed {return Err(LocalHostError::InvalidRequest("only Arcee or the specialist itself may rename it".to_owned()))}
            if tx.execute("UPDATE host_agents SET name=?2 WHERE agent_id=?1 AND name=?3",params![edit.id.to_string(),edit.name,edit.expected_name]).map_err(catalog::HostCatalogError::from)?!=1{return Err(LocalHostError::AgentConflict(edit.id))}
            let parent:String=tx.query_row("SELECT created_by FROM host_agents WHERE agent_id=?1",[edit.id.to_string()],|r|r.get(0)).map_err(catalog::HostCatalogError::from)?;
            let result=BotSummary{id:edit.id,name:edit.name,created_by:AgentId::from_uuid(Uuid::parse_str(&parent).map_err(|_|LocalHostError::InvalidRequest("invalid bot creator".to_owned()))?)};
            if cancellation.is_cancelled(){return Err(LocalHostError::BotRenameCancelled)}
            tx.execute("INSERT INTO host_bot_renames VALUES(?1,?2,?3,?4)",params![operation.to_string(),actor.to_string(),request,serde_json::to_string(&result)?]).map_err(catalog::HostCatalogError::from)?;
            tx.commit().map_err(catalog::HostCatalogError::from)?;Ok(result)
        }).await?
    }
}

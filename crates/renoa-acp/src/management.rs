use std::path::Path;

use renoa_kernel::AgentId;
use renoa_local::{AgentRecord, LocalHostError};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{Config, ServerError};

/// Runs local agent management through the same Host operations used by surfaces.
///
/// # Errors
/// Returns invalid arguments, configuration, identity, or Host storage failures.
pub async fn manage_agents(arguments: &[String]) -> Result<Value, ServerError> {
    let config = Config::from_environment()?;
    let host = config.host();
    match arguments {
        [action] if action == "list" => Ok(json!({
            "host_id": host.host_id().await?,
            "agents": host.list_agents().await?,
        })),
        [action, id] if action == "show" => {
            let id = AgentId::from_uuid(parse_id(id)?);
            let agent = host.agent(id).await?.ok_or(LocalHostError::AgentNotFound(id))?;
            Ok(json!(agent))
        }
        [action, id, name, parent @ ..] if action == "ensure" && parent.len() <= 1 => {
            let record = AgentRecord {
                id: AgentId::from_uuid(parse_id(id)?),
                profile: config.profile_id().clone(),
                name: name.clone(),
                created_by: parent.first().map(|id| parse_id(id).map(AgentId::from_uuid)).transpose()?,
            };
            Ok(json!(host.ensure_agent(record).await?))
        }
        [action, agent, session, workspace] if action == "session" => {
            let session = host.ensure_agent_session(
                AgentId::from_uuid(parse_id(agent)?), Path::new(workspace), parse_id(session)?,
            ).await?;
            Ok(json!({ "session_id": session.id(), "agent_id": session.agent_id() }))
        }
        _ => Err(ServerError::InvalidRequest(
            "usage: renoa-agent agents <list|show ID|ensure ID NAME [CREATOR_ID]|session AGENT_ID SESSION_ID WORKSPACE>".to_owned(),
        )),
    }
}

fn parse_id(value: &str) -> Result<Uuid, ServerError> {
    Uuid::parse_str(value)
        .map_err(|_| ServerError::InvalidRequest("identity must be a UUID".to_owned()))
}

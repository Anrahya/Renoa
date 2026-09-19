use std::path::Path;

use renoa_kernel::AgentId;
use renoa_local::LocalHostError;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{Config, ServerError};

/// The largest page [`renoa_local::LocalHost::list_agent_definitions`] accepts.
const AGENT_PAGE: usize = 20;

/// Runs local agent management through the same Host operations used by surfaces.
///
/// # Errors
/// Returns invalid arguments, configuration, identity, or Host storage failures.
pub async fn manage_agents(arguments: &[String]) -> Result<Value, ServerError> {
    let config = Config::from_environment()?;
    let host = config.host();
    match arguments {
        [action] if action == "list" => {
            let mut agents = Vec::new();
            let mut after = None;
            loop {
                let page = host.list_agent_definitions(after, AGENT_PAGE).await?;
                let complete = page.len() < AGENT_PAGE;
                if let Some(last) = page.last() {
                    after = Some(last.id);
                }
                agents.extend(page);
                if complete {
                    break;
                }
            }
            Ok(json!({
                "host_id": host.host_id().await?,
                "agents": agents,
            }))
        }
        [action, id] if action == "show" => {
            let id = AgentId::from_uuid(parse_id(id)?);
            let agent = host
                .agent_definition(id)
                .await?
                .ok_or(LocalHostError::AgentNotFound(id))?;
            Ok(json!(agent))
        }
        [action, agent, session, workspace] if action == "session" => {
            let session = host
                .ensure_agent_session(
                    AgentId::from_uuid(parse_id(agent)?),
                    Path::new(workspace),
                    parse_id(session)?,
                )
                .await?;
            Ok(json!({ "session_id": session.id(), "agent_id": session.agent_id() }))
        }
        _ => Err(ServerError::InvalidRequest(
            "usage: renoa-agent agents <list|show ID|session AGENT_ID SESSION_ID WORKSPACE>"
                .to_owned(),
        )),
    }
}

fn parse_id(value: &str) -> Result<Uuid, ServerError> {
    Uuid::parse_str(value)
        .map_err(|_| ServerError::InvalidRequest("identity must be a UUID".to_owned()))
}

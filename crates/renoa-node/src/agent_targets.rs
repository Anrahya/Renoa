//! Every agent in the node's Host is one advertised target, `agent:<uuid>`,
//! executed in that agent's own Host workspace.

use renoa_kernel::AgentId;
use renoa_local::{LocalHost, RenoaHome};
use renoa_protocol::TargetRef;
use uuid::Uuid;

use crate::{bridge::NodeError, node_store::TargetBinding};

const PREFIX: &str = "agent:";

pub(crate) fn target(agent: AgentId) -> TargetRef {
    TargetRef::new(format!("{PREFIX}{agent}"))
}

fn agent(target: &str) -> Option<Uuid> {
    let id = Uuid::parse_str(target.strip_prefix(PREFIX)?).ok()?;
    (target == format!("{PREFIX}{id}")).then_some(id)
}

/// The targets this node currently offers, in agent identity order.
pub(crate) async fn advertised(host: &LocalHost) -> Result<Vec<TargetRef>, NodeError> {
    let agents = host
        .list_agents()
        .await
        .map_err(|error| NodeError::Store(error.to_string()))?;
    Ok(agents.into_iter().map(|agent| target(agent.id)).collect())
}

/// Proposes the binding for a task's first command. The ledger keeps an
/// existing task's recorded session instead of this fresh one.
pub(crate) fn proposed_binding(
    home: &RenoaHome,
    target: &TargetRef,
) -> Result<TargetBinding, NodeError> {
    let agent_id = agent(target.as_str()).ok_or_else(|| {
        NodeError::Protocol(format!(
            "coordinator requested `{}`, which is not a Host agent target",
            target.as_str()
        ))
    })?;
    Ok(TargetBinding {
        target: target.as_str().to_owned(),
        agent_id,
        session_id: Uuid::new_v4(),
        workspace: workspace(home, agent_id)?,
    })
}

/// Whether a durable task binding still names one Host agent and its workspace.
pub(crate) fn serves(home: &RenoaHome, binding: &TargetBinding) -> bool {
    agent(&binding.target) == Some(binding.agent_id)
        && workspace(home, binding.agent_id).is_ok_and(|path| path == binding.workspace)
}

fn workspace(home: &RenoaHome, agent_id: Uuid) -> Result<std::path::PathBuf, NodeError> {
    home.agent_workspace(&agent_id.to_string())
        .map_err(|error| NodeError::Configuration(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::agent;

    #[test]
    fn only_a_canonical_agent_target_names_an_agent() {
        let id = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";
        assert!(agent(&format!("agent:{id}")).is_some());
        assert_eq!(agent(&format!("agent:{}", id.to_uppercase())), None);
        assert_eq!(agent(id), None);
        assert_eq!(agent("workspace:example"), None);
    }
}

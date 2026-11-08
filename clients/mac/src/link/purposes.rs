//! What each agent is for, read once from the Host, as the Android app reads
//! it (`RcpAgentRepository.readPurposes`).

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use renoa_rcp_client::{ClientError, Connection};
use renoa_rcp_wire::{HostMethod, NodeId, TargetSummary};
use serde::Deserialize;
use tokio::{sync::mpsc, time::timeout};
use uuid::Uuid;

use super::Update;

/// How a target names a Host agent: this, then the agent's id.
const AGENT_TARGET: &str = "agent:";

/// How long a Host call may go unanswered. The coordinator answers within
/// 60 seconds whether or not the Host does (`HOST_CALL_TIMEOUT` in
/// renoa-control), so this waits longer, as the Android app's `hostReply`
/// does.
const HOST_REPLY_WITHIN: Duration = Duration::from_secs(75);

/// The agents whose purpose the Host has given, or said they have none.
pub(super) type Read = HashSet<(NodeId, String)>;

/// The fields of the Host's agent definition this reads.
#[derive(Deserialize)]
struct Definition {
    operational: Operational,
}

#[derive(Deserialize)]
struct Operational {
    purpose: Option<String>,
}

/// Reads the purpose of each listed Host agent not read yet and tells the
/// views of each one there is. Every call goes to the owner's one Host,
/// whichever node offers the agent. A purpose, or the Host's answer that
/// there is no such agent, is final; any other answer or refusal leaves the
/// agent for the next agent list. A call that fails or goes unanswered stops
/// the reading: the session sees for itself whether the link was lost.
pub(super) async fn read(
    connection: Connection,
    agents: Vec<TargetSummary>,
    read: Arc<Mutex<Read>>,
    updates: mpsc::UnboundedSender<Update>,
) {
    for agent in agents {
        let key = (agent.node_id, agent.target.as_str().to_owned());
        if read.lock().expect("purposes lock").contains(&key) {
            continue;
        }
        let Some(agent_id) = host_agent(agent.target.as_str()) else {
            read.lock().expect("purposes lock").insert(key);
            continue;
        };
        let path = format!("/v1/host/agents/{agent_id}");
        let called = timeout(
            HOST_REPLY_WITHIN,
            connection.call_host(HostMethod::Get, &path, None),
        )
        .await;
        let answer = match called {
            Ok(Ok(answer)) => answer,
            Ok(Err(ClientError::Rejected { .. } | ClientError::Protocol(_))) => continue,
            Ok(Err(_)) | Err(_) => return,
        };
        let purpose = match answer.status {
            200..=299 => match serde_json::from_str::<Definition>(&answer.body) {
                Ok(definition) => definition.operational.purpose.unwrap_or_default(),
                Err(_) => continue,
            },
            404 => String::new(),
            _ => continue,
        };
        read.lock().expect("purposes lock").insert(key);
        let purpose = purpose.trim();
        if !purpose.is_empty()
            && updates
                .send(Update::Purpose {
                    node_id: agent.node_id,
                    target: agent.target.as_str().to_owned(),
                    purpose: purpose.to_owned(),
                })
                .is_err()
        {
            return;
        }
    }
}

/// The Host's id for the agent `target` names, when it names one.
fn host_agent(target: &str) -> Option<Uuid> {
    Uuid::parse_str(target.strip_prefix(AGENT_TARGET)?).ok()
}

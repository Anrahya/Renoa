//! What the Host says about its agents: which one is the main agent, what
//! each is for and how each looks, as the Android app reads them
//! (`HostProfiles`, `RcpAgentRepository.readObservation`).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use renoa_rcp_client::{ClientError, Connection};
use renoa_rcp_wire::{HostMethod, NodeId, TargetSummary};
use serde::Deserialize;
use tokio::{sync::mpsc, time::timeout};
use uuid::Uuid;

use super::Update;
use crate::face::Look;

/// How a target names a Host agent: this, then the agent's id.
const AGENT_TARGET: &str = "agent:";

/// How long a Host call may go unanswered. The coordinator answers within
/// 60 seconds whether or not the Host does (`HOST_CALL_TIMEOUT` in
/// renoa-control), so this waits longer, as the Android app's `hostReply`
/// does.
const HOST_REPLY_WITHIN: Duration = Duration::from_secs(75);

/// What has been read of each agent, and the main agent last told.
#[derive(Default)]
pub(super) struct Read {
    agents: HashMap<(NodeId, String), Profile>,
    main: Option<Option<Uuid>>,
}

#[derive(Default, Clone, Copy)]
struct Profile {
    purpose: bool,
    look: bool,
}

/// The fields of the Host's observation this reads. The main agent is
/// required, so a Host that does not say is not read as naming none.
#[derive(Deserialize)]
struct Observation {
    #[serde(deserialize_with = "Option::deserialize")]
    main_agent: Option<MainAgent>,
}

#[derive(Deserialize)]
struct MainAgent {
    main_agent_id: Uuid,
}

/// The fields of the Host's agent definition this reads.
#[derive(Deserialize)]
struct Definition {
    operational: Operational,
}

#[derive(Deserialize)]
struct Operational {
    purpose: Option<String>,
}

#[derive(Deserialize)]
struct Appearance {
    look: Look,
}

/// How the Host answered a call.
enum Answer<T> {
    Said(T),
    /// There is no such thing: final, like an answer.
    Missing,
    /// Refused or not understood: asked again on the next agent list.
    Later,
    /// The call failed or went unanswered: the session sees for itself
    /// whether the link was lost.
    Lost,
}

/// When any listed agent is the Host's, tells the views which agent is the
/// main one when that changed, then reads, for each listed Host agent, what it is for and how it looks, each
/// until the Host has answered for it. Every call goes to the owner's one
/// Host, whichever node offers the agent. The Host's answer that there is no
/// such agent is final for both; any other answer or refusal leaves that
/// read for the next agent list. A call that fails or goes unanswered stops
/// the reading.
pub(super) async fn read(
    connection: Connection,
    agents: Vec<TargetSummary>,
    read: Arc<Mutex<Read>>,
    updates: mpsc::UnboundedSender<Update>,
) {
    let held = || read.lock().expect("profiles lock");
    let agents: Vec<(TargetSummary, Uuid)> = agents
        .into_iter()
        .filter_map(|agent| host_agent(agent.target.as_str()).map(|agent_id| (agent, agent_id)))
        .collect();
    // With no Host agent listed there is no main one to mark.
    if agents.is_empty() {
        return;
    }
    match ask::<Observation>(&connection, "/v1/host").await {
        Answer::Said(observation) => {
            let main = observation.main_agent.map(|main| main.main_agent_id);
            if held().main.replace(main) != Some(main) && updates.send(Update::Main(main)).is_err()
            {
                return;
            }
        }
        Answer::Missing | Answer::Later => {}
        Answer::Lost => return,
    }
    for (agent, agent_id) in agents {
        let target = agent.target.as_str().to_owned();
        let key = (agent.node_id, target.clone());
        let mut profile = held().agents.get(&key).copied().unwrap_or_default();
        let tell = |update| updates.send(update).is_ok();
        if !profile.purpose {
            match ask::<Definition>(&connection, &format!("/v1/host/agents/{agent_id}")).await {
                Answer::Said(definition) => {
                    profile.purpose = true;
                    let purpose = definition.operational.purpose.unwrap_or_default();
                    let purpose = purpose.trim();
                    if !purpose.is_empty()
                        && !tell(Update::Purpose {
                            node_id: agent.node_id,
                            target: target.clone(),
                            purpose: purpose.to_owned(),
                        })
                    {
                        return;
                    }
                }
                // No such agent: it has no look either.
                Answer::Missing => {
                    profile = Profile {
                        purpose: true,
                        look: true,
                    }
                }
                // Its look waits for its purpose, as on the phone.
                Answer::Later => continue,
                Answer::Lost => return,
            }
            held().agents.insert(key.clone(), profile);
        }
        if !profile.look {
            let path = format!("/v1/host/agents/{agent_id}/appearance");
            match ask::<Appearance>(&connection, &path).await {
                Answer::Said(appearance) => {
                    profile.look = true;
                    held().agents.insert(key, profile);
                    if !tell(Update::Look {
                        node_id: agent.node_id,
                        target,
                        look: appearance.look,
                    }) {
                        return;
                    }
                }
                Answer::Missing => {
                    profile.look = true;
                    held().agents.insert(key, profile);
                }
                Answer::Later => {}
                Answer::Lost => return,
            }
        }
    }
}

/// Asks the Host for `path` and reads its answer as a `T`.
async fn ask<T: for<'de> Deserialize<'de>>(connection: &Connection, path: &str) -> Answer<T> {
    let called = timeout(
        HOST_REPLY_WITHIN,
        connection.call_host(HostMethod::Get, path, None),
    )
    .await;
    let answer = match called {
        Ok(Ok(answer)) => answer,
        Ok(Err(ClientError::Rejected { .. } | ClientError::Protocol(_))) => return Answer::Later,
        Ok(Err(_)) | Err(_) => return Answer::Lost,
    };
    match answer.status {
        200..=299 => serde_json::from_str(&answer.body).map_or(Answer::Later, Answer::Said),
        404 => Answer::Missing,
        _ => Answer::Later,
    }
}

/// The Host's id for the agent `target` names, when it names one.
pub fn host_agent(target: &str) -> Option<Uuid> {
    Uuid::parse_str(target.strip_prefix(AGENT_TARGET)?).ok()
}

#[cfg(test)]
mod tests {
    use super::Observation;

    #[test]
    fn an_observation_must_say_whether_there_is_a_main_agent() {
        let read = |body: &str| {
            serde_json::from_str::<Observation>(body).map(|seen| seen.main_agent.is_some())
        };
        assert!(
            read("{}").is_err(),
            "a Host that does not say is not read as naming none"
        );
        assert_eq!(read(r#"{"main_agent":null}"#).ok(), Some(false));
        let named = r#"{"main_agent":{"main_agent_id":"1b4e28ba-2fa1-11d2-883f-0016d3cca427"}}"#;
        assert_eq!(read(named).ok(), Some(true));
    }
}

use std::path::Path;

use renoa_kernel::{OperationObservation, OperationStatus, SessionId};
use serde::Serialize;
use uuid::Uuid;

use super::{HostCatalogError, ObservedAgent, parse_id};
use crate::{
    LocalHostError,
    host_storage::{KERNEL_DATABASE, MANIFEST_FILE, read_manifest_file},
};

#[derive(Debug, Serialize)]
pub struct ObservedSession {
    pub id: Uuid,
    pub agent_id: Option<Uuid>,
    #[serde(flatten)]
    pub state: ObservedSessionState,
}

#[derive(Debug, Serialize)]
#[serde(tag = "observation", rename_all = "snake_case")]
pub enum ObservedSessionState {
    Available {
        event_count: u64,
        queued_operations: u64,
        active_operation: Option<ObservedOperation>,
        latest_operation: Option<ObservedOperation>,
    },
    Unavailable {
        reason: String,
    },
}

#[derive(Debug, Serialize)]
pub struct ObservedOperation {
    pub id: Uuid,
    pub command_id: Uuid,
    pub position: u64,
    pub state: ObservedOperationState,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedOperationState {
    Queued,
    Unfinished,
    OutcomeUnknown,
    Waiting,
    Completed,
    Failed,
    Cancelled,
}

pub(super) fn read(
    root: &Path,
    agents: &mut Vec<ObservedAgent>,
) -> Result<Vec<ObservedSession>, LocalHostError> {
    let mut items = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let Some(id) = entry
            .file_name()
            .to_str()
            .and_then(|s| Uuid::parse_str(s).ok())
        else {
            continue;
        };
        let mut item = ObservedSession {
            id,
            agent_id: None,
            state: ObservedSessionState::Unavailable {
                reason: "session metadata unavailable".to_owned(),
            },
        };
        let result = (|| {
            if !entry.file_type()?.is_dir() {
                return Err(LocalHostError::InvalidRequest(
                    "published session is not a directory".to_owned(),
                ));
            }
            let manifest = read_manifest_file(&entry.path().join(MANIFEST_FILE))?;
            if manifest.session_id != SessionId::from_uuid(id) {
                return Err(LocalHostError::InvalidRequest(
                    "session directory and manifest identities differ".to_owned(),
                ));
            }
            let agent_id = parse_id(&manifest.agent_id.to_string())?;
            if let Some(agent) = agents.iter().find(|a| a.id == agent_id) {
                if agent.profile != manifest.profile.as_str() {
                    return Err(LocalHostError::InvalidRequest(
                        "session profile and agent catalog differ".to_owned(),
                    ));
                }
            } else {
                // Older sessions predate the agent catalog. Project their identity
                // without the importing writes performed by LocalHost::list_agents.
                agents.push(ObservedAgent {
                    id: agent_id,
                    profile: manifest.profile.to_string(),
                    name: manifest.profile.to_string(),
                    created_by: None,
                });
            }
            item.agent_id = Some(agent_id);
            let snapshot = renoa_kernel::observe_session(
                &entry.path().join(KERNEL_DATABASE),
                manifest.session_id,
            )
            .map_err(crate::LocalSessionError::from)?;
            if snapshot.agent_id != manifest.agent_id {
                return Err(LocalHostError::InvalidRequest(
                    "kernel and manifest agent identities differ".to_owned(),
                ));
            }
            Ok(ObservedSessionState::Available {
                event_count: snapshot.event_count,
                queued_operations: snapshot.queued_operations,
                active_operation: snapshot
                    .active_operation
                    .as_ref()
                    .map(operation)
                    .transpose()?,
                latest_operation: snapshot
                    .latest_operation
                    .as_ref()
                    .map(operation)
                    .transpose()?,
            })
        })();
        item.state = match result {
            Ok(state) => state,
            Err(error) => ObservedSessionState::Unavailable {
                reason: error.to_string(),
            },
        };
        items.push(item);
    }
    items.sort_by_key(|s| s.id);
    Ok(items)
}

fn operation(value: &OperationObservation) -> Result<ObservedOperation, LocalHostError> {
    let state = match value.status {
        OperationStatus::Queued => ObservedOperationState::Queued,
        OperationStatus::Running => ObservedOperationState::Unfinished,
        OperationStatus::OutcomeUnknown => ObservedOperationState::OutcomeUnknown,
        OperationStatus::Waiting => ObservedOperationState::Waiting,
        OperationStatus::Completed => ObservedOperationState::Completed,
        OperationStatus::Failed => ObservedOperationState::Failed,
        OperationStatus::Cancelled => ObservedOperationState::Cancelled,
        _ => {
            return Err(
                HostCatalogError::Invalid("unsupported kernel operation state".to_owned()).into(),
            );
        }
    };
    Ok(ObservedOperation {
        id: parse_id(&value.operation_id.to_string())?,
        command_id: parse_id(&value.command_id.to_string())?,
        position: value.position,
        state,
    })
}

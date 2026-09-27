//! Nodes advertise the agents they can execute; the owning principal opens
//! tasks against those advertised targets at runtime.
//!
//! Advertisements belong to one live node connection and disappear with it.
//! A task opening is durable and idempotent on its task identity: an exact
//! retry converges on the original task even after its node goes offline.

use std::{collections::BTreeSet, sync::Arc};

use renoa_protocol::{PrincipalId, TargetRef};
use rusqlite::{OptionalExtension, TransactionBehavior};
use uuid::Uuid;

use crate::{
    ControlError, NodeId, TargetSummary, TaskId, TaskSpec, control_log,
    control_schema::open_connection,
    coordinator::CoordinatorState,
    node_ownership::node_owner,
    store::{ControlStore, blocking, id_error, insert_task, json_error, sqlite_error},
};

const MAX_ADVERTISED_TARGETS: usize = 256;
const MAX_TARGET_BYTES: usize = 256;

/// Whether the requested target is executable at the moment of opening.
enum Availability {
    Advertised,
    NodeOffline,
    NotAdvertised,
}

enum Opening {
    Opened,
    Existing,
}

/// Replaces the targets advertised by the node's current connection.
pub(crate) async fn advertise_targets(
    state: &CoordinatorState,
    node_id: NodeId,
    connection_id: Uuid,
    targets: Vec<TargetRef>,
) -> Result<(), ControlError> {
    let targets = validate_targets(targets)?;
    let names = targets
        .iter()
        .map(TargetRef::as_str)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let mut nodes = state.nodes.lock().await;
    let node = nodes
        .get_mut(&node_id)
        .filter(|node| node.connection_id == connection_id)
        .ok_or_else(|| ControlError::invalid("node connection has been replaced"))?;
    node.targets = targets;
    drop(nodes);
    control_log::event(
        "info",
        "node_targets_advertised",
        &serde_json::json!({ "node_id": node_id, "targets": names }),
    );
    Ok(())
}

fn validate_targets(targets: Vec<TargetRef>) -> Result<Vec<TargetRef>, ControlError> {
    if targets.len() > MAX_ADVERTISED_TARGETS {
        return Err(ControlError::invalid(format!(
            "a node may advertise at most {MAX_ADVERTISED_TARGETS} targets"
        )));
    }
    let mut seen = BTreeSet::new();
    for target in &targets {
        let name = target.as_str();
        if name.is_empty() || name.len() > MAX_TARGET_BYTES || name.chars().any(char::is_control) {
            return Err(ControlError::invalid(format!(
                "an advertised target must be 1 to {MAX_TARGET_BYTES} bytes without control characters"
            )));
        }
        if !seen.insert(name) {
            return Err(ControlError::invalid(format!(
                "target `{name}` was advertised more than once"
            )));
        }
    }
    let mut targets = targets;
    targets.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    Ok(targets)
}

/// Lists the advertised targets of every online node the principal owns.
pub(crate) async fn list_targets(
    state: &CoordinatorState,
    principal_id: PrincipalId,
) -> Result<Vec<TargetSummary>, ControlError> {
    let owned = state.store.owned_nodes(principal_id).await?;
    let nodes = state.nodes.lock().await;
    let mut targets = owned
        .into_iter()
        .filter_map(|node_id| nodes.get(&node_id).map(|node| (node_id, node)))
        .flat_map(|(node_id, node)| {
            node.targets
                .iter()
                .cloned()
                .map(move |target| TargetSummary { node_id, target })
        })
        .collect::<Vec<_>>();
    drop(nodes);
    targets.sort_by(|left, right| {
        (left.node_id.to_string(), left.target.as_str())
            .cmp(&(right.node_id.to_string(), right.target.as_str()))
    });
    Ok(targets)
}

/// Opens one task on an advertised target, or converges on an exact retry.
pub(crate) async fn open_task(
    state: &CoordinatorState,
    task: TaskSpec,
) -> Result<(), ControlError> {
    let availability = match state.nodes.lock().await.get(&task.node_id) {
        None => Availability::NodeOffline,
        Some(node) if node.targets.contains(&task.target) => Availability::Advertised,
        Some(_) => Availability::NotAdvertised,
    };
    let fields = serde_json::json!({
        "task_id": task.task_id,
        "principal_id": task.principal_id,
        "node_id": task.node_id,
        "target": task.target.as_str(),
    });
    match state.store.open_task(task, availability).await {
        Ok(opening) => {
            let name = match opening {
                Opening::Opened => "task_opened",
                Opening::Existing => "task_open_replayed",
            };
            control_log::event("info", name, &fields);
            Ok(())
        }
        Err(error) => {
            control_log::event(
                "warn",
                "task_open_rejected",
                &serde_json::json!({ "task": fields, "error": error.to_string() }),
            );
            Err(error)
        }
    }
}

impl ControlStore {
    pub(crate) async fn owned_nodes(
        &self,
        principal_id: PrincipalId,
    ) -> Result<Vec<NodeId>, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let connection = open_connection(&path)?;
            let mut statement = connection
                .prepare("SELECT node_id FROM node_owners WHERE principal_id = ?1")
                .map_err(sqlite_error)?;
            let rows = statement
                .query_map([principal_id.to_string()], |row| row.get::<_, String>(0))
                .map_err(sqlite_error)?;
            let mut nodes = Vec::new();
            for row in rows {
                nodes.push(NodeId::from_uuid(
                    row.map_err(sqlite_error)?.parse().map_err(id_error)?,
                ));
            }
            Ok(nodes)
        })
        .await
    }

    async fn open_task(
        &self,
        task: TaskSpec,
        availability: Availability,
    ) -> Result<Opening, ControlError> {
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            let target_json = serde_json::to_string(&task.target).map_err(json_error)?;
            let existing = transaction
                .query_row(
                    "SELECT principal_id, node_id, target_json FROM tasks WHERE task_id = ?1",
                    [task.task_id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(sqlite_error)?;
            // A durable opening outranks current availability so an uncertain
            // retry observes its original result.
            if let Some(existing) = existing {
                if existing
                    == (
                        task.principal_id.to_string(),
                        task.node_id.to_string(),
                        target_json,
                    )
                {
                    return Ok(Opening::Existing);
                }
                return Err(task_in_use(task.task_id));
            }
            if node_owner(&transaction, task.node_id)? != Some(task.principal_id) {
                return Err(ControlError::not_found(format!(
                    "node {} was not found",
                    task.node_id
                )));
            }
            match availability {
                Availability::Advertised => {}
                Availability::NodeOffline => return Err(ControlError::node_offline()),
                Availability::NotAdvertised => {
                    return Err(ControlError::not_found(format!(
                        "node {} does not advertise target `{}`",
                        task.node_id,
                        task.target.as_str()
                    )));
                }
            }
            insert_task(&transaction, &task)?;
            transaction.commit().map_err(sqlite_error)?;
            Ok(Opening::Opened)
        })
        .await
    }
}

fn task_in_use(task_id: TaskId) -> ControlError {
    ControlError::conflict(format!(
        "task id {task_id} is already used by a different task"
    ))
}

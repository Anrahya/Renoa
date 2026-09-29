//! Deletes the conversation of a deleted automation: the RCP task named by the
//! automation's id, and this node's copy of it with the Host session it ran in.
//!
//! The Host marks the automation once its data is purged; the mark goes only
//! after both sides are gone, so a failure part way is retried on the next
//! pass. An automation that answered in a conversation it was created in never
//! had a task of its own; deleting its absent task succeeds and the mark goes.

use renoa_control::TaskId;
use renoa_kernel::AgentId;
use renoa_local::AutomationScheduler;
use renoa_rcp_client::{ClientError, Connection};
use uuid::Uuid;

use crate::{
    bridge::{NodeError, NodeRuntime},
    node_log,
};

/// Deletes every marked conversation it can. Returns only a lost coordinator
/// connection; any other failure is logged and retried on the next pass.
pub(crate) async fn delete_conversations(
    runtime: &NodeRuntime,
    scheduler: &AutomationScheduler,
    connection: &Connection,
) -> Result<(), NodeError> {
    let automations = match scheduler.conversations_to_delete().await {
        Ok(automations) => automations,
        Err(error) => {
            failed(None, &error.to_string());
            return Ok(());
        }
    };
    for automation in automations {
        match delete_conversation(runtime, scheduler, connection, automation).await {
            Ok(()) => {}
            Err(Failure::Transport(error)) => return Err(error.into()),
            Err(Failure::Other(reason)) => failed(Some(automation), &reason),
        }
    }
    Ok(())
}

enum Failure {
    Transport(ClientError),
    Other(String),
}

async fn delete_conversation(
    runtime: &NodeRuntime,
    scheduler: &AutomationScheduler,
    connection: &Connection,
    automation: Uuid,
) -> Result<(), Failure> {
    let task_id = TaskId::from_uuid(automation);
    // The coordinator refuses while an execution is unfinished, so nothing
    // local is removed before it agrees.
    match connection.delete_task(task_id).await {
        Ok(()) => {}
        Err(error @ ClientError::Transport(_)) => return Err(Failure::Transport(error)),
        Err(error) => return Err(Failure::Other(error.to_string())),
    }
    let other = |error: &dyn std::fmt::Display| Failure::Other(error.to_string());
    let session = runtime
        .state
        .task_session(task_id)
        .await
        .map_err(|error| other(&error))?;
    if let Some((agent, session)) = session {
        runtime
            .host
            .delete_session(AgentId::from_uuid(agent), session)
            .await
            .map_err(|error| other(&error))?;
        runtime
            .state
            .forget_task(task_id)
            .await
            .map_err(|error| other(&error))?;
    }
    scheduler
        .conversation_deleted(automation)
        .await
        .map_err(|error| other(&error))?;
    node_log::event(
        "info",
        "automation_conversation_deleted",
        &serde_json::json!({
            "automation_id": automation,
            "task_id": task_id,
            "session_id": session.map(|(_, session)| session),
        }),
    );
    Ok(())
}

fn failed(automation: Option<Uuid>, reason: &str) {
    node_log::event(
        "warn",
        "automation_conversation_deletion_failed",
        &serde_json::json!({ "automation_id": automation, "error": reason }),
    );
}

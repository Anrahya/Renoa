//! Which principal owns each execution node.
//!
//! Ownership decides who may open new tasks on a node. A node without a
//! recorded owner keeps serving the tasks an operator created for it, but no
//! surface can open another task there.

use std::{sync::Arc, time::SystemTime};

use renoa_protocol::PrincipalId;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{
    ControlError, EnrollmentToken, NodeId, PeerIdentity,
    control_schema::open_connection,
    identity_store::PendingEnrollment,
    store::{ControlStore, blocking, id_error, sqlite_error},
};

pub(crate) fn create_schema(connection: &Connection) -> Result<(), ControlError> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS node_owners (
                node_id TEXT PRIMARY KEY,
                principal_id TEXT NOT NULL
            );",
        )
        .map_err(sqlite_error)
}

/// Adopts the owner of every node whose existing tasks all belong to one
/// principal. Nodes shared by several principals stay without an owner.
pub(crate) fn adopt_task_owners(connection: &Connection) -> Result<(), ControlError> {
    connection
        .execute(
            "INSERT OR IGNORE INTO node_owners (node_id, principal_id)
             SELECT node_id, MIN(principal_id) FROM tasks
             GROUP BY node_id
             HAVING COUNT(DISTINCT principal_id) = 1",
            [],
        )
        .map_err(sqlite_error)?;
    Ok(())
}

pub(crate) fn node_owner(
    connection: &Connection,
    node_id: NodeId,
) -> Result<Option<PrincipalId>, ControlError> {
    connection
        .query_row(
            "SELECT principal_id FROM node_owners WHERE node_id = ?1",
            [node_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(sqlite_error)?
        .map(|owner| Ok(PrincipalId::from_uuid(owner.parse().map_err(id_error)?)))
        .transpose()
}

impl ControlStore {
    /// Records the node's owner and its single-use enrollment together, so a
    /// refused owner leaves no enrollment and a failed enrollment no owner.
    pub(crate) async fn create_node_enrollment(
        &self,
        node_id: NodeId,
        owner: PrincipalId,
        expires_at: SystemTime,
    ) -> Result<EnrollmentToken, ControlError> {
        let (token, enrollment) =
            PendingEnrollment::new(&PeerIdentity::Node { node_id }, expires_at)?;
        let path = Arc::clone(&self.path);
        blocking(move || {
            let mut connection = open_connection(&path)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sqlite_error)?;
            match node_owner(&transaction, node_id)? {
                Some(existing) if existing != owner => {
                    return Err(ControlError::conflict(format!(
                        "node {node_id} already belongs to another principal"
                    )));
                }
                Some(_) => {}
                None => {
                    transaction
                        .execute(
                            "INSERT INTO node_owners (node_id, principal_id) VALUES (?1, ?2)",
                            params![node_id.to_string(), owner.to_string()],
                        )
                        .map_err(sqlite_error)?;
                }
            }
            enrollment.insert(&transaction)?;
            transaction.commit().map_err(sqlite_error)
        })
        .await?;
        Ok(token)
    }
}

#[cfg(test)]
mod tests;

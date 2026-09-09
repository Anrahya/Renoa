use renoa_kernel::AgentId;
use rusqlite::{OptionalExtension as _, Transaction, params};
use uuid::Uuid;

use super::{RoutineError, RoutineRecord, store};
use crate::host::catalog::HostCatalogError;

// Separate owner receipts preserve the foreign key and authority of historical
// agent receipts. Both are committed by the same routine mutation transaction.
pub(super) fn initialize(tx: &Transaction<'_>) -> Result<(), HostCatalogError> {
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS host_routine_owner_mutations (
        operation_id TEXT PRIMARY KEY, principal_id TEXT NOT NULL,
        request_json TEXT NOT NULL CHECK(json_valid(request_json)),
        result_json TEXT NOT NULL CHECK(json_valid(result_json))
    ) STRICT;",
    )?;
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) enum RoutineActor {
    Agent(AgentId),
    Owner { host_id: Uuid, principal: Uuid },
}

impl RoutineActor {
    pub fn authorize(self, tx: &Transaction<'_>, target: AgentId) -> Result<(), RoutineError> {
        match self {
            Self::Agent(id) => store::authorize(tx, id, target),
            Self::Owner { .. } => Ok(()),
        }
    }

    pub fn replay(
        self,
        tx: &Transaction<'_>,
        operation: Uuid,
        request: &str,
    ) -> Result<Option<RoutineRecord>, RoutineError> {
        let (sql, identity) = match self {
            Self::Agent(id) => (
                "SELECT actor_id,request_json,result_json FROM host_routine_mutations WHERE operation_id=?1",
                id.to_string(),
            ),
            Self::Owner { host_id, principal } => {
                let stored: String = tx.query_row(
                    "SELECT host_id FROM host_identity WHERE singleton=1",
                    [],
                    |r| r.get(0),
                )?;
                if stored != host_id.to_string() {
                    return Err(HostCatalogError::Invalid(
                        "Host identity changed; reconnect explicitly".to_owned(),
                    )
                    .into());
                }
                (
                    "SELECT principal_id,request_json,result_json FROM host_routine_owner_mutations WHERE operation_id=?1",
                    principal.to_string(),
                )
            }
        };
        let receipt: Option<(String, String, String)> = tx
            .query_row(sql, [operation.to_string()], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?;
        receipt
            .map(|(actor, original, result)| {
                if actor != identity || original != request {
                    return Err(RoutineError::Conflict);
                }
                Ok(serde_json::from_str(&result)?)
            })
            .transpose()
    }

    pub fn save(
        self,
        tx: &Transaction<'_>,
        operation: Uuid,
        request: &str,
        result: &RoutineRecord,
    ) -> Result<(), RoutineError> {
        let (sql, identity) = match self {
            Self::Agent(id) => (
                "INSERT INTO host_routine_mutations(operation_id,actor_id,request_json,result_json) VALUES(?1,?2,?3,?4)",
                id.to_string(),
            ),
            Self::Owner { principal, .. } => (
                "INSERT INTO host_routine_owner_mutations(operation_id,principal_id,request_json,result_json) VALUES(?1,?2,?3,?4)",
                principal.to_string(),
            ),
        };
        tx.execute(
            sql,
            params![
                operation.to_string(),
                identity,
                request,
                serde_json::to_string(result)?
            ],
        )?;
        Ok(())
    }
}

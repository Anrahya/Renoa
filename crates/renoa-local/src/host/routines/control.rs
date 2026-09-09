use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{RoutineError, RoutineMutation, RoutineRecord, receipts::RoutineActor, store};
use crate::{HostCatalogError, HostObserver, LocalHostError, host::catalog};

/// One logical request. Retry with this same identity and revision after an
/// uncertain response; a new identity represents a new operation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutineEnablement {
    pub operation_id: Uuid,
    pub expected_revision: i64,
    pub enabled: bool,
}

/// Owner controls for existing routines, without constructing an execution Host.
/// The trusted adapter authenticates the principal; request bodies cannot supply
/// caller identity. Agent tools use `LocalHost`'s restricted agent API instead.
#[derive(Clone)]
pub struct HostRoutineControl {
    database: PathBuf,
    host_id: Uuid,
    owner: Uuid,
}

impl HostRoutineControl {
    /// Pins existing storage and its configured human owner. Does not migrate,
    /// initialize a Host, discover models, or acquire execution ownership.
    /// # Errors
    /// Returns missing/incompatible storage or a different Host identity.
    pub fn open(root: &Path, host_id: Uuid, owner: Uuid) -> Result<Self, LocalHostError> {
        let root = std::fs::canonicalize(root)?;
        if HostObserver::open(&root)?.host_id() != host_id {
            return Err(HostCatalogError::Invalid(
                "configured Host identity does not match storage".to_owned(),
            )
            .into());
        }
        Ok(Self {
            database: root.join(catalog::HOST_DATABASE),
            host_id,
            owner,
        })
    }

    /// Pauses future admissions or resumes using the existing schedule rules.
    /// Already-admitted work is unchanged. The returned record is the committed
    /// receipt, which can be older than subsequent edits. Cancelling this future
    /// does not prove rollback: retry the identical request to recover its receipt.
    /// # Errors
    /// Rejects other owners, replaced Hosts, stale revisions, deleted routines,
    /// expired one-time schedules when resuming, and storage failures.
    pub async fn set_enabled(
        &self,
        authenticated_principal: Uuid,
        routine: Uuid,
        request: RoutineEnablement,
        now_ms: i64,
    ) -> Result<RoutineRecord, LocalHostError> {
        if authenticated_principal != self.owner {
            return Err(RoutineError::Forbidden.into());
        }
        let database = self.database.clone();
        let actor = RoutineActor::Owner {
            host_id: self.host_id,
            principal: self.owner,
        };
        Ok(tokio::task::spawn_blocking(move || {
            store::mutate(
                &database,
                actor,
                request.operation_id,
                RoutineMutation::SetEnabled {
                    id: routine,
                    expected_revision: request.expected_revision,
                    enabled: request.enabled,
                },
                now_ms,
                &CancellationToken::new(),
            )
        })
        .await??)
    }
}

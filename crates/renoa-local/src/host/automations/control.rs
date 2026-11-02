use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    AutomationError, AutomationMutation, AutomationRecord, receipts::AutomationActor, store,
};
use crate::{HostCatalogError, HostObserver, LocalHostError, host::catalog};

/// One logical request. Retry with this same identity and revision after an
/// uncertain response; a new identity represents a new operation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationEnablement {
    pub operation_id: Uuid,
    pub expected_revision: i64,
    pub enabled: bool,
}

/// One owner request to run an automation now. Retry with this same identity
/// after an uncertain response; it becomes the run's identity.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutomationRunRequest {
    pub operation_id: Uuid,
}

/// Owner controls for existing automations, without constructing an execution Host.
/// The trusted adapter authenticates the principal; request bodies cannot supply
/// caller identity. Agent tools use `LocalHost`'s restricted agent API instead.
#[derive(Clone)]
pub struct HostAutomationControl {
    database: PathBuf,
    host_id: Uuid,
    owner: Uuid,
}

impl HostAutomationControl {
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
    /// Rejects other owners, replaced Hosts, stale revisions, deleted automations,
    /// expired one-time schedules when resuming, and storage failures.
    pub async fn set_enabled(
        &self,
        authenticated_principal: Uuid,
        automation: Uuid,
        request: AutomationEnablement,
        now_ms: i64,
    ) -> Result<AutomationRecord, LocalHostError> {
        self.mutate(
            authenticated_principal,
            request.operation_id,
            AutomationMutation::SetEnabled {
                id: automation,
                expected_revision: request.expected_revision,
                enabled: request.enabled,
            },
            now_ms,
        )
        .await
    }

    /// Admits a run of `automation` now, under the request's operation ID,
    /// as an agent's `run_now` does, whether or not the automation is paused.
    /// The returned record is the committed receipt.
    /// # Errors
    /// Rejects other owners, replaced Hosts, deleted automations, an
    /// automation with a run already admitted, and storage failures.
    pub async fn run_now(
        &self,
        authenticated_principal: Uuid,
        automation: Uuid,
        request: AutomationRunRequest,
        now_ms: i64,
    ) -> Result<AutomationRecord, LocalHostError> {
        self.mutate(
            authenticated_principal,
            request.operation_id,
            AutomationMutation::RunNow { id: automation },
            now_ms,
        )
        .await
    }

    async fn mutate(
        &self,
        authenticated_principal: Uuid,
        operation: Uuid,
        mutation: AutomationMutation,
        now_ms: i64,
    ) -> Result<AutomationRecord, LocalHostError> {
        if authenticated_principal != self.owner {
            return Err(AutomationError::Forbidden.into());
        }
        let database = self.database.clone();
        let actor = AutomationActor::Owner {
            host_id: self.host_id,
            principal: self.owner,
        };
        Ok(tokio::task::spawn_blocking(move || {
            store::mutate(
                &database,
                actor,
                operation,
                mutation,
                now_ms,
                &CancellationToken::new(),
            )
        })
        .await??)
    }
}

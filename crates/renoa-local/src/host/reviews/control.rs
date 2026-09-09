//! Authenticated owner policy edits using the existing review transaction rules.
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use rusqlite::{OptionalExtension as _, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{GitHubReviewError, GitHubReviewRepository, GitHubReviewTrigger, catalog, store};
use crate::{HostCatalogError, HostObserver, LocalHostError};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewPolicyUpdate {
    pub operation_id: Uuid,
    pub expected_revision: i64,
    pub enabled: bool,
    pub triggers: BTreeSet<GitHubReviewTrigger>,
    pub skip_drafts: bool,
}

#[derive(Clone)]
pub struct HostReviewControl {
    database: PathBuf,
    host_id: Uuid,
    owner: Uuid,
}

impl HostReviewControl {
    /// Pins existing storage and owner without starting an execution runtime.
    /// # Errors
    /// Returns missing/incompatible storage or a mismatched Host identity.
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

    /// Changes admission policy for an existing repository. Agent, installation
    /// and repository identity cannot be changed through this operation.
    /// The receipt can predate later edits; refresh observation for current state.
    /// # Errors
    /// Rejects another owner, replaced Host, stale revision, reused operation ID,
    /// missing repository or invalid request. Cancellation does not prove rollback.
    pub async fn update_policy(
        &self,
        authenticated_principal: Uuid,
        repository_id: i64,
        request: ReviewPolicyUpdate,
    ) -> Result<GitHubReviewRepository, LocalHostError> {
        if authenticated_principal != self.owner {
            return Err(GitHubReviewError::Forbidden.into());
        }
        let control = self.clone();
        Ok(tokio::task::spawn_blocking(move || control.update(repository_id, &request)).await??)
    }

    fn update(
        &self,
        repository_id: i64,
        request: &ReviewPolicyUpdate,
    ) -> Result<GitHubReviewRepository, GitHubReviewError> {
        if request.operation_id.is_nil() || request.expected_revision < 1 {
            return Err(GitHubReviewError::Invalid(
                "provide an operation ID and current policy revision".to_owned(),
            ));
        }
        let mut db = catalog::open_verified(&self.database)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored: String = tx.query_row(
            "SELECT host_id FROM host_identity WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        if stored != self.host_id.to_string() {
            return Err(HostCatalogError::Invalid(
                "Host identity changed; reconnect explicitly".to_owned(),
            )
            .into());
        }
        // Trusted-local commands use bare UUID keys. Owner keys are disjoint and
        // bind authority; owner input can never replay a local command receipt.
        let key = format!("owner:{}:{}", self.owner, request.operation_id);
        let input = serde_json::to_string(&(repository_id, request))?;
        let prior: Option<(String, String)> = tx
            .query_row(
                "SELECT request_json,result_json FROM host_review_operations WHERE operation_id=?1",
                [&key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((previous, result)) = prior {
            if previous != input {
                return Err(GitHubReviewError::Conflict);
            }
            return Ok(serde_json::from_str(&result)?);
        }
        let mut policy = store::repository(&tx, repository_id)?
            .ok_or(GitHubReviewError::NotFound)?
            .policy;
        policy.enabled = request.enabled;
        policy.triggers.clone_from(&request.triggers);
        policy.skip_drafts = request.skip_drafts;
        let result = store::set_repository(&tx, Some(request.expected_revision), &policy)?;
        tx.execute(
            "INSERT INTO host_review_operations VALUES(?1,?2,?3)",
            params![key, input, serde_json::to_string(&result)?],
        )?;
        tx.commit()?;
        Ok(result)
    }
}

//! The owner-facing view of one person's `USER.md`.
//!
//! Agents edit a profile through `agent_documents`; this is the same edit for a
//! person who reads and changes their own profile outside a turn.

use renoa_agent::ToolErrorCode;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{LocalHost, LocalHostError};
use crate::documents::{UserProfile, read_user_profile, replace_user_profile};

/// Why a profile edit did not apply.
#[derive(Debug, thiserror::Error)]
pub enum ProfileEditError {
    /// The profile changed after the editor read it; nothing was written.
    #[error("the profile changed after it was read; read it again before editing")]
    Stale,
    /// The edit named a malformed revision, or the stored profile is unsafe:
    /// linked, not a regular file, or not UTF-8. Retrying cannot fix it.
    #[error("{0}")]
    Invalid(String),
    /// Storage failed; retrying the same edit converges.
    #[error("{0}")]
    Unavailable(String),
}

impl LocalHost {
    /// Reads the `USER.md` of `principal`. A person with no profile reads as
    /// empty, with the revision of empty content.
    ///
    /// # Errors
    ///
    /// Returns an error when the profile path is unsafe or unreadable.
    pub async fn user_profile(&self, principal: Uuid) -> Result<UserProfile, LocalHostError> {
        let data_directory = self.config.home.path().to_path_buf();
        Ok(
            tokio::task::spawn_blocking(move || read_user_profile(&data_directory, principal))
                .await??,
        )
    }

    /// Replaces the `USER.md` of `principal` against the revision last read,
    /// with the same checks an agent's edit has, and returns the new profile.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileEditError::Stale`] when the profile changed after it was
    /// read, and the other variants for invalid input or storage failures.
    pub async fn replace_user_profile(
        &self,
        principal: Uuid,
        expected_revision: &str,
        content: String,
        cancellation: &CancellationToken,
    ) -> Result<UserProfile, ProfileEditError> {
        let revision = replace_user_profile(
            self.config.home.path(),
            principal,
            expected_revision,
            &content,
            cancellation,
        )
        .await
        .map_err(|error| match error.code() {
            ToolErrorCode::Conflict => ProfileEditError::Stale,
            ToolErrorCode::InvalidInput => ProfileEditError::Invalid(error.to_string()),
            _ => ProfileEditError::Unavailable(error.to_string()),
        })?;
        Ok(UserProfile { content, revision })
    }
}

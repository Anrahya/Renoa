//! The owner-facing view of one person's `USER.md` and of the notes agents
//! keep about them.
//!
//! Agents edit a profile through `agent_documents`; this is the same edit for a
//! person who reads and changes their own profile outside a turn. The notes are
//! read only: agents write them with `memory_note`.

use renoa_agent::ToolErrorCode;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{LocalHost, LocalHostError};
use crate::{
    documents::{UserProfile, read_user_profile, replace_user_profile},
    plugins::host::memory,
};

/// How many of the newest notes [`LocalHost::memory_notes`] reads. Mnem caps a
/// note at `mnem::memory::ENTRY_CHARS` (280) bytes, so even escaped they stay
/// far below one RCP message.
pub const MEMORY_PAGE: usize = 100;

/// The notes agents keep about one person, newest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemoryNotes {
    /// The newest [`MEMORY_PAGE`] notes, without those a newer note corrects.
    pub notes: Vec<MemoryNote>,
    /// How many notes the notebook holds, older and corrected ones included.
    pub total: u64,
}

/// One note, as its agent wrote it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MemoryNote {
    pub id: u64,
    /// The day it was written, `YYYY-MM-DD`.
    pub written: String,
    /// The name of the agent that wrote it.
    pub author: Option<String>,
    /// Its words, without the tags agents write for Mnem.
    pub text: String,
    /// The day it says something happened, `YYYY-MM-DD`, when that was not
    /// the day it was written.
    pub happened: Option<String>,
    /// When it asks to be brought up, `YYYY-MM-DDTHH:MM` in the person's zone.
    pub due: Option<String>,
}

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

    /// Reads the newest notes agents keep about `principal`. A person with no
    /// notebook has none, and none is created.
    ///
    /// # Errors
    ///
    /// Returns an error when the person's directory is unsafe or the notebook
    /// cannot be read.
    pub async fn memory_notes(&self, principal: Uuid) -> Result<MemoryNotes, LocalHostError> {
        let data_directory = self.config.home.path().to_path_buf();
        let (notes, total) = tokio::task::spawn_blocking(move || {
            memory::newest(&data_directory, principal, MEMORY_PAGE)
        })
        .await?
        .map_err(|reason| LocalHostError::Io(std::io::Error::other(reason)))?;
        let notes = notes
            .into_iter()
            .rev()
            .filter(|note| note.superseded_by.is_none())
            .map(|note| MemoryNote {
                id: note.id,
                written: note.written,
                author: note.author,
                due: memory::due(&note.text),
                text: memory::readable(&note.text),
                happened: note.happened,
            })
            .collect();
        Ok(MemoryNotes { notes, total })
    }
}

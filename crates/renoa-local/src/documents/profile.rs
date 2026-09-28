//! One person's `USER.md`, shared by every agent that talks to them.
//!
//! The profile belongs to an RCP principal, not to an agent, and lives at
//! `<data directory>/users/<principal id>/USER.md`. An absent profile reads as
//! empty, so nothing is written until an agent first records a fact.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use renoa_agent::ToolError;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    Missing,
    files::{
        DocumentSnapshot, USER_FILE, canonical_data_directory, document_io, restrict_directory,
        revision,
    },
    read_snapshot, replace_document, stale_edit, validate_revision,
};
use crate::AgentDefinitionError;

const PROFILE_DIRECTORY: &str = "users";

/// One person's profile and the revision an edit must name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UserProfile {
    pub content: String,
    pub revision: String,
}

/// Reads the profile of `principal` from the Host data directory.
///
/// # Errors
///
/// Returns an error when the data directory or a profile directory is unsafe,
/// or the profile is not a readable UTF-8 regular file.
pub(crate) fn read_user_profile(
    data_directory: &Path,
    principal: Uuid,
) -> Result<UserProfile, AgentDefinitionError> {
    let snapshot =
        PersonProfile::new(&canonical_data_directory(data_directory)?, principal).read()?;
    Ok(UserProfile {
        content: snapshot.content,
        revision: snapshot.revision,
    })
}

/// Replaces the profile of `principal` against the revision its editor read.
///
/// # Errors
///
/// Returns a conflict for a stale revision, invalid input for a malformed one
/// or an unsafe profile, and an I/O error when storage fails.
pub(crate) async fn replace_user_profile(
    data_directory: &Path,
    principal: Uuid,
    expected_revision: &str,
    content: &str,
    cancellation: &CancellationToken,
) -> Result<String, ToolError> {
    let data_directory = data_directory.to_path_buf();
    let data_directory = off_executor(move || canonical_data_directory(&data_directory)).await?;
    PersonProfile::new(&data_directory, principal)
        .replace(expected_revision, content, cancellation)
        .await
}

/// The profile of the person one turn is talking to.
#[derive(Clone, Debug)]
pub(super) struct PersonProfile {
    users: PathBuf,
    directory: PathBuf,
}

impl PersonProfile {
    /// Names the profile of `principal` under a canonical Host data directory.
    pub(super) fn new(data_directory: &Path, principal: Uuid) -> Self {
        let users = data_directory.join(PROFILE_DIRECTORY);
        let directory = users.join(principal.to_string());
        Self { users, directory }
    }

    pub(super) fn path(&self) -> PathBuf {
        self.directory.join(USER_FILE)
    }

    /// Reads the profile. A person with no recorded profile reads as empty.
    ///
    /// # Errors
    ///
    /// Returns an error when a profile directory is a link or another file, or
    /// the profile is not a readable UTF-8 regular file.
    pub(super) fn read(&self) -> Result<DocumentSnapshot, AgentDefinitionError> {
        for directory in [&self.users, &self.directory] {
            if !plain_directory(directory)? {
                return Ok(empty());
            }
        }
        let path = self.path();
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(empty()),
            _ => read_snapshot(&path),
        }
    }

    /// Replaces the profile against the revision its editor last read.
    ///
    /// A first edit creates the person's private directory, so the revision is
    /// checked before that effect: a stale edit leaves nothing behind.
    pub(super) async fn replace(
        &self,
        expected_revision: &str,
        content: &str,
        cancellation: &CancellationToken,
    ) -> Result<String, ToolError> {
        validate_revision(expected_revision)?;
        let new_revision = revision(content.as_bytes());
        let profile = self.clone();
        let current = off_executor(move || profile.read()).await?.revision;
        if current == new_revision {
            return Ok(new_revision);
        }
        if current != expected_revision {
            return Err(stale_edit());
        }
        let profile = self.clone();
        off_executor(move || profile.prepare()).await?;
        replace_document(
            &self.path(),
            Missing::ReadAsEmpty,
            expected_revision,
            content,
            cancellation,
        )
        .await
    }

    /// Creates the profile directories an edit writes into and makes each one
    /// private, including one an interrupted first edit left behind.
    ///
    /// # Errors
    ///
    /// Returns an error when a directory cannot be created or restricted, or
    /// an existing one is a link or another file.
    fn prepare(&self) -> Result<(), AgentDefinitionError> {
        for directory in [&self.users, &self.directory] {
            if let Err(source) = fs::create_dir(directory)
                && source.kind() != io::ErrorKind::AlreadyExists
            {
                return Err(document_io(
                    "create user profile directory",
                    directory,
                    source,
                ));
            }
            // Refuse a link before restricting, since changing permissions
            // follows it.
            if !plain_directory(directory)? {
                return Err(AgentDefinitionError::ProfileDirectory {
                    path: directory.clone(),
                });
            }
            restrict_directory(directory)?;
        }
        Ok(())
    }
}

/// Runs blocking profile file work off the async executor.
async fn off_executor<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, AgentDefinitionError> + Send + 'static,
) -> Result<T, ToolError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| ToolError::internal(format!("user profile task failed: {error}")))?
        .map_err(|error| profile_error(&error))
}

/// Reports whether `path` is a plain directory, `false` when it is absent, and
/// an error for a link or any other file, which is never followed.
fn plain_directory(path: &Path) -> Result<bool, AgentDefinitionError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(AgentDefinitionError::ProfileDirectory {
            path: path.to_path_buf(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(document_io("inspect user profile directory", path, source)),
    }
}

fn empty() -> DocumentSnapshot {
    DocumentSnapshot {
        content: String::new(),
        revision: revision(b""),
    }
}

/// A linked, non-regular, or non-UTF-8 profile is invalid as it stands; every
/// other failure is a storage failure.
fn profile_error(error: &AgentDefinitionError) -> ToolError {
    match error {
        AgentDefinitionError::ProfileDirectory { .. }
        | AgentDefinitionError::DocumentNotFile { .. }
        | AgentDefinitionError::DocumentInvalidUtf8 { .. } => {
            ToolError::invalid_input(error.to_string())
        }
        _ => ToolError::io(error.to_string(), false),
    }
}

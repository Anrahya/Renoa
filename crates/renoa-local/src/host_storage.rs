use std::{
    fs::{File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use renoa_kernel::{AgentId, SessionId};
use serde::{Deserialize, Serialize};

use crate::{
    ALPHA_PROFILE_ID, LocalHostError, LocalSession,
    selection::{RuntimeSelection, create_selection_log},
    trace::{TRACE_DATABASE, TraceStore},
};

pub(crate) const KERNEL_DATABASE: &str = "kernel.sqlite3";
pub(crate) const MANIFEST_FILE: &str = "session.json";
const MANIFEST_VERSION: u32 = 3;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionManifest {
    version: u32,
    profile: String,
    pub(crate) agent_id: AgentId,
    pub(crate) session_id: SessionId,
    pub(crate) workspace: PathBuf,
}

#[derive(Deserialize)]
struct SessionManifestHeader {
    version: u32,
}

pub(crate) fn create_session_storage(
    sessions: &Path,
    agent_id: AgentId,
    session_id: SessionId,
    workspace: PathBuf,
    selection: &RuntimeSelection,
) -> Result<PathBuf, LocalHostError> {
    let manifest = SessionManifest {
        version: MANIFEST_VERSION,
        profile: ALPHA_PROFILE_ID.to_owned(),
        agent_id,
        session_id,
        workspace,
    };
    publish_session(sessions, session_id, |staging| {
        write_manifest(staging, &manifest)?;
        create_selection_log(staging, selection)?;
        let session = LocalSession::create(staging.join(KERNEL_DATABASE), agent_id, session_id)?;
        drop(session);
        drop(TraceStore::create(
            staging.join(TRACE_DATABASE),
            session_id,
        )?);
        Ok(())
    })
}

pub(crate) async fn read_manifest(path: PathBuf) -> Result<SessionManifest, LocalHostError> {
    tokio::task::spawn_blocking(move || read_manifest_file(&path)).await?
}

pub(crate) fn delete_session_storage(
    sessions: &Path,
    session_id: SessionId,
) -> Result<(), LocalHostError> {
    let directory = sessions.join(session_id.to_string());
    let tombstone = sessions.join(format!(".deleting-{session_id}"));
    let directory_exists = directory.try_exists()?;
    let tombstone_exists = tombstone.try_exists()?;

    if directory_exists && tombstone_exists {
        return Err(LocalHostError::InvalidRequest(
            "session storage contains both live and deleting records for the requested session"
                .to_owned(),
        ));
    }
    if !directory_exists {
        if tombstone_exists {
            remove_tombstone(sessions, &tombstone)?;
        }
        return Ok(());
    }

    require_directory(&directory)?;
    let manifest = read_manifest_file(&directory.join(MANIFEST_FILE))?;
    if manifest.session_id != session_id {
        return Err(LocalHostError::InvalidRequest(
            "session metadata does not match the requested deletion".to_owned(),
        ));
    }
    let kernel_path = directory.join(KERNEL_DATABASE);
    require_file(&kernel_path)?;
    let owner = LocalSession::load(&kernel_path, session_id)?;
    if owner.agent_id() != manifest.agent_id {
        return Err(LocalHostError::InvalidRequest(
            "session metadata differs from its kernel agent binding".to_owned(),
        ));
    }

    std::fs::rename(&directory, &tombstone)?;
    File::open(sessions)?.sync_all()?;
    drop(owner);
    remove_tombstone(sessions, &tombstone)
}

fn publish_session(
    sessions: &Path,
    session_id: SessionId,
    initialize: impl FnOnce(&Path) -> Result<(), LocalHostError>,
) -> Result<PathBuf, LocalHostError> {
    let final_directory = sessions.join(session_id.to_string());
    let staging = sessions.join(format!(".creating-{session_id}"));
    std::fs::create_dir(&staging)?;
    restrict_session_directory(&staging)?;
    let mut published = false;
    let result = initialize(&staging).and_then(|()| {
        File::open(&staging)?.sync_all()?;
        std::fs::rename(&staging, &final_directory)?;
        published = true;
        File::open(sessions)?.sync_all()?;
        Ok(final_directory.clone())
    });
    match result {
        Ok(directory) => Ok(directory),
        Err(source) if published => Err(source),
        Err(source) => match std::fs::remove_dir_all(&staging) {
            Ok(()) => Err(source),
            Err(cleanup) => Err(LocalHostError::SessionCreationCleanup {
                source: Box::new(source),
                cleanup,
            }),
        },
    }
}

#[cfg(unix)]
fn restrict_session_directory(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn restrict_session_directory(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

fn write_manifest(directory: &Path, manifest: &SessionManifest) -> Result<(), LocalHostError> {
    let bytes = serde_json::to_vec(manifest)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join(MANIFEST_FILE))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

fn read_manifest_file(path: &Path) -> Result<SessionManifest, LocalHostError> {
    require_file(path)?;
    let bytes = std::fs::read(path)?;
    let header = serde_json::from_slice::<SessionManifestHeader>(&bytes)?;
    if header.version != MANIFEST_VERSION {
        return Err(LocalHostError::InvalidRequest(format!(
            "session storage version {} is unsupported; expected {MANIFEST_VERSION}",
            header.version
        )));
    }
    let manifest = serde_json::from_slice::<SessionManifest>(&bytes)?;
    if manifest.profile != ALPHA_PROFILE_ID {
        return Err(LocalHostError::InvalidRequest(
            "session metadata does not describe Renoa Alpha".to_owned(),
        ));
    }
    Ok(manifest)
}

fn remove_tombstone(sessions: &Path, tombstone: &Path) -> Result<(), LocalHostError> {
    match std::fs::remove_dir_all(tombstone) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    File::open(sessions)?.sync_all()?;
    Ok(())
}

fn require_directory(path: &Path) -> Result<(), LocalHostError> {
    if std::fs::symlink_metadata(path)?.file_type().is_dir() {
        Ok(())
    } else {
        Err(LocalHostError::InvalidRequest(format!(
            "session storage path is not a directory: {}",
            path.display()
        )))
    }
}

fn require_file(path: &Path) -> Result<(), LocalHostError> {
    if std::fs::symlink_metadata(path)?.file_type().is_file() {
        Ok(())
    } else {
        Err(LocalHostError::InvalidRequest(format!(
            "session storage path is not a regular file: {}",
            path.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use renoa_kernel::{AgentId, KernelError, SessionId};
    use tempfile::tempdir;

    use super::{KERNEL_DATABASE, create_session_storage, delete_session_storage, publish_session};
    use crate::{
        LocalHostError, LocalSession, LocalSessionError, ModelProvider, ReasoningLevel,
        selection::RuntimeSelection,
    };

    #[test]
    fn failed_initialization_never_publishes_a_partial_session() {
        let directory = tempdir().expect("temporary directory");
        let session_id = renoa_kernel::SessionId::new();

        let result = publish_session(directory.path(), session_id, |staging| {
            std::fs::write(staging.join("partial"), "not a session")?;
            Err(LocalHostError::Io(io::Error::other(
                "injected creation failure",
            )))
        });

        assert!(matches!(result, Err(LocalHostError::Io(_))));
        assert!(!directory.path().join(session_id.to_string()).exists());
        assert!(
            !directory
                .path()
                .join(format!(".creating-{session_id}"))
                .exists()
        );
    }

    #[cfg(unix)]
    #[test]
    fn published_session_directory_is_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempdir().expect("temporary directory");
        let session_id = renoa_kernel::SessionId::new();

        let published = publish_session(directory.path(), session_id, |_| Ok(()))
            .expect("publish session directory");

        assert_eq!(
            std::fs::metadata(published)
                .expect("session metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn deletion_requires_exclusive_ownership_and_is_idempotent() {
        let sessions = tempdir().expect("temporary directory");
        let workspace = tempdir().expect("workspace directory");
        let agent_id = AgentId::new();
        let session_id = SessionId::new();
        let directory = create_session_storage(
            sessions.path(),
            agent_id,
            session_id,
            workspace.path().to_owned(),
            &RuntimeSelection {
                provider: ModelProvider::Xai,
                model: "test".to_owned(),
                reasoning: ReasoningLevel::High,
            },
        )
        .expect("create session storage");
        let owner = LocalSession::load(directory.join(KERNEL_DATABASE), session_id)
            .expect("own kernel session");

        let active_delete = delete_session_storage(sessions.path(), session_id);
        assert!(matches!(
            active_delete,
            Err(LocalHostError::Session(LocalSessionError::Kernel(
                KernelError::AlreadyRunning { .. }
            )))
        ));
        assert!(directory.is_dir());

        drop(owner);
        delete_session_storage(sessions.path(), session_id).expect("delete session storage");
        assert!(!directory.exists());
        assert!(
            !sessions
                .path()
                .join(format!(".deleting-{session_id}"))
                .exists()
        );

        delete_session_storage(sessions.path(), session_id)
            .expect("repeat session deletion idempotently");
    }

    #[test]
    fn deletion_retry_cleans_a_published_tombstone() {
        let sessions = tempdir().expect("temporary directory");
        let session_id = SessionId::new();
        let directory = sessions.path().join(session_id.to_string());
        let tombstone = sessions.path().join(format!(".deleting-{session_id}"));
        std::fs::create_dir(&directory).expect("create session directory");
        std::fs::write(directory.join("data"), "durable").expect("write session data");
        std::fs::rename(&directory, &tombstone).expect("publish deletion tombstone");

        delete_session_storage(sessions.path(), session_id).expect("resume session deletion");

        assert!(!directory.exists());
        assert!(!tombstone.exists());
    }
}

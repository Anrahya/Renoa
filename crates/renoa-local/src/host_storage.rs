use std::{
    fs::{File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use renoa_kernel::{AgentId, KernelError, SessionId};
use serde::{Deserialize, Serialize};

use crate::{
    AgentProfileId, LocalHostError, LocalSession, LocalSessionError,
    selection::{RuntimeSelection, create_selection_log},
    trace::{TRACE_DATABASE, TraceStore},
};

pub(crate) const KERNEL_DATABASE: &str = "kernel.sqlite3";
pub(crate) const MANIFEST_FILE: &str = "session.json";
const MANIFEST_VERSION: u32 = 3;
const CREATION_LOCK_FILE: &str = ".session-creation.lock";
const OWNERSHIP_HANDOFF_TIMEOUT: Duration = Duration::from_millis(100);
const OWNERSHIP_HANDOFF_POLL: Duration = Duration::from_millis(1);

pub(crate) enum SessionPublication {
    Created(PathBuf),
    Existing,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionManifest {
    version: u32,
    pub(crate) profile: AgentProfileId,
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
    profile: AgentProfileId,
    agent_id: AgentId,
    session_id: SessionId,
    workspace: PathBuf,
    selection: &RuntimeSelection,
) -> Result<SessionPublication, LocalHostError> {
    let manifest = SessionManifest {
        version: MANIFEST_VERSION,
        profile,
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
            agent_id,
            &manifest.profile,
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
    let owner = load_session_after_handoff(&kernel_path, session_id)?;
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

/// Opens a session after a local owner was just closed.
///
/// A concurrently forked child briefly inherits the kernel lock descriptor
/// until `exec` applies close-on-exec. This bounded wait covers that OS
/// handoff. A live Renoa owner still wins and returns `AlreadyRunning`.
pub(crate) fn load_session_after_handoff(
    kernel_path: &Path,
    session_id: SessionId,
) -> Result<LocalSession, LocalHostError> {
    let started = Instant::now();
    loop {
        match LocalSession::load(kernel_path, session_id) {
            Err(LocalSessionError::Kernel(KernelError::AlreadyRunning { .. }))
                if started.elapsed() < OWNERSHIP_HANDOFF_TIMEOUT =>
            {
                std::thread::sleep(OWNERSHIP_HANDOFF_POLL);
            }
            result => return result.map_err(Into::into),
        }
    }
}

fn publish_session(
    sessions: &Path,
    session_id: SessionId,
    initialize: impl FnOnce(&Path) -> Result<(), LocalHostError>,
) -> Result<SessionPublication, LocalHostError> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(sessions.join(CREATION_LOCK_FILE))?;
    lock.lock()?;
    let final_directory = sessions.join(session_id.to_string());
    if final_directory.try_exists()? {
        require_directory(&final_directory)?;
        return Ok(SessionPublication::Existing);
    }
    let staging = sessions.join(format!(".creating-{session_id}"));
    remove_stale_staging(&staging)?;
    std::fs::create_dir(&staging)?;
    restrict_session_directory(&staging)?;
    let mut published = false;
    let result = initialize(&staging).and_then(|()| {
        File::open(&staging)?.sync_all()?;
        std::fs::rename(&staging, &final_directory)?;
        published = true;
        File::open(sessions)?.sync_all()?;
        Ok(SessionPublication::Created(final_directory.clone()))
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

fn remove_stale_staging(staging: &Path) -> Result<(), LocalHostError> {
    match std::fs::symlink_metadata(staging) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            std::fs::remove_dir_all(staging)?;
            Ok(())
        }
        Ok(_) => Err(LocalHostError::InvalidRequest(format!(
            "session creation staging path is not a directory: {}",
            staging.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
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
    Ok(serde_json::from_slice::<SessionManifest>(&bytes)?)
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
    use std::{
        cell::Cell,
        io,
        sync::mpsc::{self, RecvTimeoutError},
        thread,
        time::Duration,
    };

    use renoa_kernel::{AgentId, KernelError, SessionId};
    use tempfile::tempdir;

    use super::{
        KERNEL_DATABASE, SessionPublication, create_session_storage, delete_session_storage,
        load_session_after_handoff, publish_session,
    };
    use crate::{
        ALPHA_PROFILE_ID, AgentProfileId, LocalHostError, LocalSessionError, ModelProvider,
        ReasoningLevel, selection::RuntimeSelection,
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

        let SessionPublication::Created(published) =
            publish_session(directory.path(), session_id, |_| Ok(()))
                .expect("publish session directory")
        else {
            panic!("new session unexpectedly existed");
        };

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
    fn publication_recovers_a_stale_creation_and_reuses_the_published_session() {
        let sessions = tempdir().expect("temporary directory");
        let session_id = SessionId::new();
        let staging = sessions.path().join(format!(".creating-{session_id}"));
        std::fs::create_dir(&staging).expect("create stale staging directory");
        std::fs::write(staging.join("partial"), "incomplete").expect("write stale data");

        let SessionPublication::Created(published) =
            publish_session(sessions.path(), session_id, |directory| {
                assert!(!directory.join("partial").exists());
                std::fs::write(directory.join("complete"), "ready")?;
                Ok(())
            })
            .expect("recover session publication")
        else {
            panic!("stale creation unexpectedly resolved as published");
        };
        assert_eq!(
            std::fs::read_to_string(published.join("complete")).expect("read published data"),
            "ready"
        );

        let initialized = Cell::new(false);
        let publication = publish_session(sessions.path(), session_id, |_| {
            initialized.set(true);
            Ok(())
        })
        .expect("reuse published session");
        assert!(matches!(publication, SessionPublication::Existing));
        assert!(!initialized.get());
    }

    #[test]
    fn deletion_requires_exclusive_ownership_and_is_idempotent() {
        let sessions = tempdir().expect("temporary directory");
        let workspace = tempdir().expect("workspace directory");
        let agent_id = AgentId::new();
        let session_id = SessionId::new();
        let SessionPublication::Created(directory) = create_session_storage(
            sessions.path(),
            AgentProfileId::new(ALPHA_PROFILE_ID).expect("Alpha profile id"),
            agent_id,
            session_id,
            workspace.path().to_owned(),
            &RuntimeSelection {
                provider: ModelProvider::Xai,
                model: "test".to_owned(),
                reasoning: ReasoningLevel::High,
            },
        )
        .expect("create session storage") else {
            panic!("new session unexpectedly existed");
        };
        let owner = load_session_after_handoff(&directory.join(KERNEL_DATABASE), session_id)
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
    fn ownership_handoff_waits_briefly_for_a_released_local_owner() {
        let sessions = tempdir().expect("temporary directory");
        let workspace = tempdir().expect("workspace directory");
        let agent_id = AgentId::new();
        let session_id = SessionId::new();
        let SessionPublication::Created(directory) = create_session_storage(
            sessions.path(),
            AgentProfileId::new(ALPHA_PROFILE_ID).expect("Alpha profile id"),
            agent_id,
            session_id,
            workspace.path().to_owned(),
            &RuntimeSelection {
                provider: ModelProvider::Xai,
                model: "test".to_owned(),
                reasoning: ReasoningLevel::High,
            },
        )
        .expect("create session storage") else {
            panic!("new session unexpectedly existed");
        };
        let kernel_path = directory.join(KERNEL_DATABASE);
        let owner =
            load_session_after_handoff(&kernel_path, session_id).expect("own published session");
        let (sender, receiver) = mpsc::sync_channel(1);
        let waiter = thread::spawn(move || {
            sender
                .send(load_session_after_handoff(&kernel_path, session_id))
                .expect("send handoff result");
        });

        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(20)),
            Err(RecvTimeoutError::Timeout)
        ));
        drop(owner);
        let reopened = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("handoff completed")
            .expect("reopen after ownership release");
        assert_eq!(reopened.agent_id(), agent_id);
        drop(reopened);
        waiter.join().expect("handoff thread completed");
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

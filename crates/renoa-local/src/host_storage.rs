use std::{
    fs::{File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use renoa_kernel::{AgentId, KernelError, SessionId};
use serde::{Deserialize, Serialize};

use crate::{
    LocalHostError, LocalSession, LocalSessionError,
    selection::{RuntimeSelection, create_selection_log},
    trace::{TRACE_DATABASE, TraceStore},
};

pub(crate) const KERNEL_DATABASE: &str = "kernel.sqlite3";
pub(crate) const MANIFEST_FILE: &str = "session.json";
const MANIFEST_VERSION: u32 = 4;
const LIFECYCLE_LOCK_FILE: &str = ".session-creation.lock";
const OWNERSHIP_HANDOFF_TIMEOUT: Duration = Duration::from_millis(100);
const OWNERSHIP_HANDOFF_POLL: Duration = Duration::from_millis(1);

pub(crate) enum SessionPublication {
    Created(OpenedSessionStorage),
    Existing(OpenedSessionStorage),
}

enum DirectoryPublication {
    Created(PathBuf),
    Existing,
}

pub(crate) struct OpenedSessionStorage {
    pub(crate) directory: PathBuf,
    pub(crate) manifest: SessionManifest,
    pub(crate) kernel: LocalSession,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionManifest {
    version: u32,
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
) -> Result<SessionPublication, LocalHostError> {
    create_session_storage_with_hook(sessions, agent_id, session_id, workspace, selection, || {})
}

fn create_session_storage_with_hook(
    sessions: &Path,
    agent_id: AgentId,
    session_id: SessionId,
    workspace: PathBuf,
    selection: &RuntimeSelection,
    after_publish: impl FnOnce(),
) -> Result<SessionPublication, LocalHostError> {
    let manifest = SessionManifest {
        version: MANIFEST_VERSION,
        agent_id,
        session_id,
        workspace,
    };
    let _lifecycle = session_lifecycle_lock(sessions)?;
    let publication = publish_session_locked(sessions, session_id, |staging| {
        write_manifest(staging, &manifest)?;
        create_selection_log(staging, selection)?;
        let session = LocalSession::create(staging.join(KERNEL_DATABASE), agent_id, session_id)?;
        drop(session);
        drop(TraceStore::create(
            staging.join(TRACE_DATABASE),
            session_id,
            agent_id,
        )?);
        Ok(())
    })?;
    match publication {
        DirectoryPublication::Created(directory) => {
            after_publish();
            let kernel = load_session_after_handoff(&directory.join(KERNEL_DATABASE), session_id)?;
            Ok(SessionPublication::Created(OpenedSessionStorage {
                directory,
                manifest,
                kernel,
            }))
        }
        DirectoryPublication::Existing => {
            open_session_storage_locked(sessions, agent_id, session_id, &manifest.workspace, || {})
                .map(SessionPublication::Existing)
        }
    }
}

pub(crate) fn open_session_storage(
    sessions: &Path,
    expected_agent: AgentId,
    session_id: SessionId,
    workspace: &Path,
) -> Result<OpenedSessionStorage, LocalHostError> {
    open_session_storage_with_hook(sessions, expected_agent, session_id, workspace, || {})
}

pub(crate) async fn read_manifest(path: PathBuf) -> Result<SessionManifest, LocalHostError> {
    tokio::task::spawn_blocking(move || read_manifest_file(&path)).await?
}

fn open_session_storage_with_hook(
    sessions: &Path,
    expected_agent: AgentId,
    session_id: SessionId,
    workspace: &Path,
    after_manifest: impl FnOnce(),
) -> Result<OpenedSessionStorage, LocalHostError> {
    let _lifecycle = session_lifecycle_lock(sessions)?;
    open_session_storage_locked(
        sessions,
        expected_agent,
        session_id,
        workspace,
        after_manifest,
    )
}

fn open_session_storage_locked(
    sessions: &Path,
    expected_agent: AgentId,
    session_id: SessionId,
    workspace: &Path,
    after_manifest: impl FnOnce(),
) -> Result<OpenedSessionStorage, LocalHostError> {
    let directory = sessions.join(session_id.to_string());
    require_directory(&directory)?;
    let manifest = read_manifest_file(&directory.join(MANIFEST_FILE))?;
    after_manifest();
    if manifest.agent_id != expected_agent {
        return Err(LocalHostError::InvalidRequest(
            "session belongs to a different agent".to_owned(),
        ));
    }
    if manifest.session_id != session_id {
        return Err(LocalHostError::InvalidRequest(
            "session metadata does not match the requested Agent session".to_owned(),
        ));
    }
    let requested_workspace = std::fs::canonicalize(workspace)?;
    if manifest.workspace != requested_workspace {
        return Err(LocalHostError::InvalidRequest(
            "session workspace differs from its durable binding".to_owned(),
        ));
    }
    let kernel = LocalSession::load(directory.join(KERNEL_DATABASE), session_id)?;
    if kernel.agent_id() != manifest.agent_id {
        return Err(LocalHostError::InvalidRequest(
            "session metadata differs from its kernel agent binding".to_owned(),
        ));
    }
    Ok(OpenedSessionStorage {
        directory,
        manifest,
        kernel,
    })
}

/// Removes one session directory when its manifest binds it to `agent_id`.
///
/// A missing directory or published tombstone succeeds for any agent, because
/// no manifest survives to compare.
pub(crate) fn delete_session_storage(
    sessions: &Path,
    agent_id: AgentId,
    session_id: SessionId,
) -> Result<(), LocalHostError> {
    delete_session_storage_with_hook(sessions, agent_id, session_id, || {})
}

fn delete_session_storage_with_hook(
    sessions: &Path,
    agent_id: AgentId,
    session_id: SessionId,
    after_manifest: impl FnOnce(),
) -> Result<(), LocalHostError> {
    let _lifecycle = session_lifecycle_lock(sessions)?;
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
    after_manifest();
    if manifest.agent_id != agent_id {
        return Err(LocalHostError::InvalidRequest(
            "session belongs to a different agent".to_owned(),
        ));
    }
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

#[cfg(test)]
fn publish_session(
    sessions: &Path,
    session_id: SessionId,
    initialize: impl FnOnce(&Path) -> Result<(), LocalHostError>,
) -> Result<DirectoryPublication, LocalHostError> {
    let _lifecycle = session_lifecycle_lock(sessions)?;
    publish_session_locked(sessions, session_id, initialize)
}

fn publish_session_locked(
    sessions: &Path,
    session_id: SessionId,
    initialize: impl FnOnce(&Path) -> Result<(), LocalHostError>,
) -> Result<DirectoryPublication, LocalHostError> {
    let final_directory = sessions.join(session_id.to_string());
    if final_directory.try_exists()? {
        require_directory(&final_directory)?;
        return Ok(DirectoryPublication::Existing);
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
        Ok(DirectoryPublication::Created(final_directory.clone()))
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

fn session_lifecycle_lock(sessions: &Path) -> Result<File, LocalHostError> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(sessions.join(LIFECYCLE_LOCK_FILE))?;
    lock.lock()?;
    Ok(lock)
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

pub(crate) fn read_manifest_file(path: &Path) -> Result<SessionManifest, LocalHostError> {
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
mod tests;

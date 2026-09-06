use std::{
    fs::File,
    path::{Path, PathBuf},
    time::Duration,
};

use renoa_agent::ToolError;
use sha2::{Digest as _, Sha256};
use std::fmt::Write as _;
use tokio_util::sync::CancellationToken;

use crate::tool_error::io_error;

/// Owns the stable sidecar inode, never the inode replaced by rename.
/// Sidecars must not be unlinked: doing so would split cooperating owners.
/// This coordinates Renoa writers, not arbitrary external file modifications.
pub(crate) struct FileUpdate {
    pub(crate) path: PathBuf,
    _lock: File,
}

impl FileUpdate {
    pub(crate) async fn acquire(
        path: &Path,
        cancellation: &CancellationToken,
    ) -> Result<Self, ToolError> {
        let path = path.to_path_buf();
        let (path, lock) = tokio::task::spawn_blocking(move || open_lock(&path))
            .await
            .map_err(|error| ToolError::internal(format!("file lock task failed: {error}")))?
            .map_err(|error| io_error("open file update lock", &error, false))?;
        loop {
            if cancellation.is_cancelled() {
                return Err(ToolError::cancelled("file update was cancelled", false));
            }
            match lock.try_lock() {
                Ok(()) => return Ok(Self { path, _lock: lock }),
                Err(std::fs::TryLockError::WouldBlock) => {
                    #[cfg(test)]
                    if let Ok(notify) = probes::CONTENDED.try_with(std::sync::Arc::clone) {
                        notify.notify_one();
                    }
                    tokio::select! {
                        biased;
                        () = cancellation.cancelled() => {
                            return Err(ToolError::cancelled("file update was cancelled", false));
                        }
                        () = tokio::time::sleep(Duration::from_millis(10)) => {}
                    }
                }
                Err(std::fs::TryLockError::Error(error)) => {
                    return Err(io_error("lock file update", &error, false));
                }
            }
        }
    }
}

fn open_lock(path: &Path) -> std::io::Result<(PathBuf, File)> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("file has no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("file has no name"))?;
    let parent = std::fs::canonicalize(parent)?;
    let mut lock_name = String::from(".renoa-lock-");
    for byte in Sha256::digest(name.as_encoded_bytes()) {
        write!(&mut lock_name, "{byte:02x}").map_err(std::io::Error::other)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    Ok((parent.join(name), options.open(parent.join(lock_name))?))
}

#[cfg(test)]
pub(crate) mod probes {
    use std::sync::Arc;
    use tokio::sync::Notify;
    tokio::task_local! {
        pub(crate) static CONTENDED: Arc<Notify>;
        pub(crate) static CHECKED: (Arc<Notify>, Arc<Notify>);
    }

    pub(crate) async fn after_check() {
        if let Ok((checked, release)) = CHECKED.try_with(Clone::clone) {
            checked.notify_one();
            release.notified().await;
        }
    }
}

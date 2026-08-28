use std::{
    fs::{File, OpenOptions},
    path::PathBuf,
    time::Duration,
};

use tokio_util::sync::CancellationToken;

use crate::mcp::{McpHostError, McpOAuthError, hex_sha256};

const POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(super) struct OAuthLock {
    _file: File,
}

pub(super) async fn acquire(
    root: PathBuf,
    connection_id: &str,
    wait: Duration,
    cancellation: &CancellationToken,
) -> Result<OAuthLock, McpHostError> {
    let lock_name = format!("{}.lock", hex_sha256(connection_id.as_bytes()));
    let file = tokio::task::spawn_blocking(move || {
        prepare_directory(&root)?;
        open_lock(&root.join(lock_name))
    })
    .await??;
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(OAuthLock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(error)) => return Err(McpHostError::Io(error)),
        }
        tokio::select! {
            () = cancellation.cancelled() => return Err(McpOAuthError::Cancelled.into()),
            () = tokio::time::sleep_until(deadline) => {
                return Err(McpOAuthError::InProgress(connection_id.to_owned()).into());
            }
            () = tokio::time::sleep(POLL_INTERVAL) => {}
        }
    }
}

fn prepare_directory(path: &std::path::Path) -> Result<(), McpHostError> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn open_lock(path: &std::path::Path) -> Result<File, McpHostError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

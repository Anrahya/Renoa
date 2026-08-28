use std::{path::Path, time::Duration};

use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::{
    mcp::{McpHostError, McpOAuthError},
    process::{child_pid_raw, configure_process_group, stop_process_group_raw},
};

const OPEN_DEADLINE: Duration = Duration::from_secs(10);

pub(super) async fn open(
    executable: &Path,
    url: &str,
    cancellation: &CancellationToken,
) -> Result<(), McpHostError> {
    let mut command = Command::new(executable);
    command
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    configure_process_group(&mut command);
    let mut child = command
        .spawn()
        .map_err(|source| McpOAuthError::Browser { source })?;
    let pid = match child_pid_raw(&child) {
        Ok(pid) => pid,
        Err(error) => {
            child
                .kill()
                .await
                .map_err(|source| McpOAuthError::Browser { source })?;
            return Err(McpOAuthError::Browser {
                source: std::io::Error::other(error.to_string()),
            }
            .into());
        }
    };
    let signal = tokio::select! {
        biased;
        () = cancellation.cancelled() => None,
        () = tokio::time::sleep(OPEN_DEADLINE) => None,
        status = child.wait() => Some(status),
    };
    match signal {
        Some(Ok(status)) if status.success() => Ok(()),
        Some(Ok(status)) => Err(McpOAuthError::BrowserStatus {
            status: status.to_string(),
        }
        .into()),
        Some(Err(source)) => Err(McpOAuthError::Browser { source }.into()),
        None => {
            stop_process_group_raw(&mut child, pid)
                .await
                .map_err(|error| McpOAuthError::BrowserStatus {
                    status: format!("browser cleanup failed: {error}"),
                })?;
            if cancellation.is_cancelled() {
                Err(McpOAuthError::Cancelled.into())
            } else {
                Err(McpOAuthError::BrowserStatus {
                    status: "browser command exceeded 10 seconds".to_owned(),
                }
                .into())
            }
        }
    }
}

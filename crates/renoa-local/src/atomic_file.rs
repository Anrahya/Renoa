use std::{io, path::Path};

use renoa_agent::ToolError;
use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;
use tokio_util::sync::CancellationToken;

use crate::tool_error::io_error;

pub(crate) type ContentHash = [u8; 32];

pub(crate) fn content_hash(content: &[u8]) -> ContentHash {
    Sha256::digest(content).into()
}

/// Atomically replaces one file and proves the parent directory was synced.
///
/// Cancellation before rename leaves the target unchanged. Once rename starts,
/// the operation waits for a definite result. A post-rename durability failure
/// is reported as outcome-unknown rather than as a false definite failure.
pub(crate) async fn replace(
    path: &Path,
    content: &[u8],
    expected: Option<ContentHash>,
    cancellation: &CancellationToken,
) -> Result<(), ToolError> {
    if cancellation.is_cancelled() {
        return Err(ToolError::cancelled("file update was cancelled", false));
    }
    let parent = path
        .parent()
        .ok_or_else(|| ToolError::invalid_input("file path has no parent directory"))?;
    let permissions = target_permissions(path).await?;
    let (temporary, mut file) = create_temporary(path, permissions).await?;
    if let Err(error) = write_and_sync(&mut file, content, cancellation).await {
        drop(file);
        return Err(cleanup_error(&temporary, error).await);
    }
    if cancellation.is_cancelled() {
        drop(file);
        return Err(cleanup_error(
            &temporary,
            ToolError::cancelled("file update was cancelled", false),
        )
        .await);
    }
    if let Some(expected) = expected
        && let Err(error) = verify_expected_content(path, expected).await
    {
        drop(file);
        return Err(cleanup_error(&temporary, error).await);
    }
    if cancellation.is_cancelled() {
        drop(file);
        return Err(cleanup_error(
            &temporary,
            ToolError::cancelled("file update was cancelled", false),
        )
        .await);
    }
    drop(file);
    if let Err(error) = tokio::fs::rename(&temporary, path).await {
        return Err(cleanup_error(&temporary, io_error("commit file update", &error, false)).await);
    }
    sync_parent(parent).await
}

async fn target_permissions(path: &Path) -> Result<Option<std::fs::Permissions>, ToolError> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) => Ok(Some(metadata.permissions())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error("inspect target file", &error, false)),
    }
}

async fn create_temporary(
    path: &Path,
    permissions: Option<std::fs::Permissions>,
) -> Result<(std::path::PathBuf, tokio::fs::File), ToolError> {
    let temporary = temporary_path(path);
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .await
        .map_err(|error| io_error("create temporary file", &error, false))?;
    if let Some(permissions) = permissions
        && let Err(error) = file.set_permissions(permissions).await
    {
        drop(file);
        return Err(cleanup_error(
            &temporary,
            io_error("preserve file permissions", &error, false),
        )
        .await);
    }
    Ok((temporary, file))
}

async fn write_and_sync(
    file: &mut tokio::fs::File,
    content: &[u8],
    cancellation: &CancellationToken,
) -> Result<(), ToolError> {
    let write_result = {
        let write = file.write_all(content);
        tokio::pin!(write);
        tokio::select! {
            biased;
            () = cancellation.cancelled() => None,
            result = &mut write => Some(result),
        }
    };
    match write_result {
        None => return Err(ToolError::cancelled("file update was cancelled", false)),
        Some(Err(error)) => return Err(io_error("write temporary file", &error, false)),
        Some(Ok(())) => {}
    }
    file.sync_all()
        .await
        .map_err(|error| io_error("sync temporary file", &error, false))
}

async fn verify_expected_content(path: &Path, expected: ContentHash) -> Result<(), ToolError> {
    let current = tokio::fs::read(path)
        .await
        .map_err(|error| io_error("recheck edited file", &error, false))?;
    if content_hash(&current) == expected {
        Ok(())
    } else {
        Err(ToolError::conflict(
            "file changed after it was read; edit was not applied",
        ))
    }
}

async fn sync_parent(parent: &Path) -> Result<(), ToolError> {
    let parent = parent.to_path_buf();
    match tokio::task::spawn_blocking(move || std::fs::File::open(parent)?.sync_all()).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(ToolError::outcome_unknown(format!(
            "file was replaced, but parent directory durability could not be confirmed: {error}"
        ))),
        Err(error) => Err(ToolError::outcome_unknown(format!(
            "file was replaced, but its durability check did not complete: {error}"
        ))),
    }
}

fn temporary_path(path: &Path) -> std::path::PathBuf {
    let name = path.file_name().map_or_else(
        || "file".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    path.with_file_name(format!(".{name}.renoa-{}.tmp", uuid::Uuid::new_v4()))
}

async fn cleanup_error(path: &Path, original: ToolError) -> ToolError {
    match tokio::fs::remove_file(path).await {
        Ok(()) => original,
        Err(error) if error.kind() == io::ErrorKind::NotFound => original,
        Err(error) => ToolError::io(
            format!("{original}; temporary file cleanup also failed: {error}"),
            false,
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn replacement_is_atomic_preserves_permissions_and_syncs() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("target.txt");
        std::fs::write(&path, "before").expect("write target");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))
            .expect("set permissions");

        replace(&path, b"after", None, &CancellationToken::new())
            .await
            .expect("replace file");

        assert_eq!(
            std::fs::read_to_string(&path).expect("read target"),
            "after"
        );
        assert_eq!(
            std::fs::metadata(&path)
                .expect("target metadata")
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
        assert!(
            std::fs::read_dir(directory.path())
                .expect("read directory")
                .all(|entry| !entry
                    .expect("directory entry")
                    .file_name()
                    .to_string_lossy()
                    .contains(".renoa-"))
        );
    }

    #[tokio::test]
    async fn content_precondition_rejects_a_stale_edit_without_touching_target() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("target.txt");
        std::fs::write(&path, "current").expect("write target");

        let error = replace(
            &path,
            b"edited",
            Some(content_hash(b"stale")),
            &CancellationToken::new(),
        )
        .await
        .expect_err("stale edit must fail");

        assert_eq!(error.code(), renoa_agent::ToolErrorCode::Conflict);
        assert_eq!(
            std::fs::read_to_string(&path).expect("read target"),
            "current"
        );
    }
}

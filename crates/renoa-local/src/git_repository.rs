//! Read-only Git evidence shared by workspace tools and their callers.
//! No provider, agent identity, hosting service or review policy belongs here.
use std::{
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio_util::sync::CancellationToken;

mod process;
#[cfg(test)]
pub(crate) mod tests;
pub(crate) mod tools;

#[derive(Clone)]
pub(crate) struct GitRepository {
    directory: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitChange {
    pub path: String,
    pub previous_path: Option<String>,
    pub status: String,
}

impl GitRepository {
    pub(crate) fn open(root: &Path) -> io::Result<Self> {
        let root = std::fs::canonicalize(root)?;
        let directory = metadata_directory(&root)?;
        Ok(Self { directory })
    }

    pub(crate) async fn changes(
        &self,
        base: &str,
        head: &str,
        cancel: &CancellationToken,
    ) -> io::Result<Vec<GitChange>> {
        revision(base)?;
        revision(head)?;
        let mut command = self.command();
        command.args([
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--find-renames",
            "--name-status",
            "-z",
            base,
            head,
            "--",
        ]);
        process::read(command, cancel, async |stdout| {
            let mut reader = BufReader::new(stdout);
            let mut changes = Vec::new();
            while let Some(status) = field(&mut reader).await? {
                let first = required_field(&mut reader).await?;
                let (path, previous_path) = if status.starts_with(['R', 'C']) {
                    (required_field(&mut reader).await?, Some(first))
                } else {
                    (first, None)
                };
                changes.push(GitChange {
                    path,
                    previous_path,
                    status,
                });
            }
            Ok((changes, false))
        })
        .await
    }

    pub(crate) async fn diff(
        &self,
        base: &str,
        head: &str,
        path: &str,
        offset: u64,
        cancel: &CancellationToken,
    ) -> io::Result<process::GitPage> {
        let command = self.diff_command(base, head, path, cancel).await?;
        process::page(command, offset, cancel).await
    }

    pub(crate) async fn show(
        &self,
        commit: &str,
        path: &str,
        offset: u64,
        cancel: &CancellationToken,
    ) -> io::Result<process::GitPage> {
        let command = self.show_command(commit, path)?;
        process::page(command, offset, cancel).await
    }

    pub(crate) async fn contains(
        &self,
        commit: &str,
        path: &str,
        cancel: &CancellationToken,
    ) -> io::Result<bool> {
        revision(commit)?;
        relative(path)?;
        let mut command = self.command();
        command.args(["ls-tree", "-z", commit, "--", path]);
        process::read(command, cancel, async |stdout| {
            let mut reader = BufReader::new(stdout);
            let entry = field(&mut reader).await?;
            Ok((
                entry
                    .as_ref()
                    .is_some_and(|entry| entry.split_whitespace().nth(1) == Some("blob")),
                false,
            ))
        })
        .await
    }

    fn show_command(&self, commit: &str, path: &str) -> io::Result<tokio::process::Command> {
        revision(commit)?;
        relative(path)?;
        let mut command = self.command();
        command.args(["cat-file", "blob", &format!("{commit}:{path}")]);
        Ok(command)
    }

    async fn diff_command(
        &self,
        base: &str,
        head: &str,
        path: &str,
        cancel: &CancellationToken,
    ) -> io::Result<tokio::process::Command> {
        revision(base)?;
        revision(head)?;
        relative(path)?;
        let mut command = self.command();
        // Literal paths, no external drivers/textconv, and no subprocesses from
        // repository configuration. Rename evidence is read from both paths.
        command.args([
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--find-renames",
            "--unified=3",
            base,
            head,
            "--",
            path,
        ]);
        for change in self.changes(base, head, cancel).await? {
            if change.path == path {
                if let Some(previous) = change.previous_path {
                    command.arg(previous);
                }
            } else if change.previous_path.as_deref() == Some(path) {
                command.arg(change.path);
            }
        }
        Ok(command)
    }

    fn command(&self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new("git");
        command.current_dir(&self.directory);
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(["--no-pager", "--literal-pathspecs", "--git-dir"])
            .arg(&self.directory)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "diff.external=",
                "-c",
                "core.quotePath=false",
            ]);
        command
    }
}

fn metadata_directory(root: &Path) -> io::Result<PathBuf> {
    let entry = root.join(".git");
    let metadata = std::fs::symlink_metadata(&entry)?;
    if metadata.is_dir() {
        return std::fs::canonicalize(entry);
    }
    if metadata.is_file() {
        let pointer = std::fs::read_to_string(&entry)?;
        if let Some(path) = pointer.trim_end().strip_prefix("gitdir: ") {
            let directory = std::fs::canonicalize(root.join(path))?;
            // A linked worktree owns a metadata directory in the main repo.
            // Require Git's reciprocal pointer so arbitrary .git files cannot
            // silently bind this workspace to some unrelated repository.
            let back = std::fs::read_to_string(directory.join("gitdir"))?;
            if std::fs::canonicalize(directory.join(back.trim_end()))? == entry {
                return Ok(directory);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "workspace must own its Git directory or registered linked-worktree metadata",
    ))
}

fn revision(value: &str) -> io::Result<()> {
    if !matches!(value.len(), 40 | 64) || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "use a full immutable Git commit ID",
        ));
    }
    Ok(())
}

pub(crate) fn relative(value: &str) -> io::Result<()> {
    if value.is_empty()
        || value.contains('\0')
        || Path::new(value)
            .components()
            .any(|p| !matches!(p, std::path::Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "use a repository-relative path",
        ));
    }
    Ok(())
}

async fn field(reader: &mut BufReader<tokio::process::ChildStdout>) -> io::Result<Option<String>> {
    let mut bytes = Vec::new();
    if reader.read_until(0, &mut bytes).await? == 0 {
        return Ok(None);
    }
    if bytes.pop() != Some(0) {
        return Err(io::Error::other("incomplete Git inventory"));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

async fn required_field(reader: &mut BufReader<tokio::process::ChildStdout>) -> io::Result<String> {
    field(reader)
        .await?
        .ok_or_else(|| io::Error::other("incomplete Git inventory"))
}

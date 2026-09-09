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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitSide {
    Base,
    #[default]
    Head,
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

    pub(crate) async fn has_line(
        &self,
        commit: &str,
        path: &str,
        line: u32,
        cancel: &CancellationToken,
    ) -> io::Result<bool> {
        if line == 0 || !self.contains(commit, path, cancel).await? {
            return Ok(false);
        }
        let command = self.show_command(commit, path)?;
        process::read(command, cancel, async |stdout| {
            let mut reader = BufReader::new(stdout);
            for _ in 1..line {
                if !process::skip_line(&mut reader).await? {
                    return Ok((false, false));
                }
            }
            Ok((process::line_prefix(&mut reader).await?.is_some(), true))
        })
        .await
    }

    pub(crate) async fn in_diff(
        &self,
        base: &str,
        head: &str,
        path: &str,
        side: GitSide,
        line: u32,
        cancel: &CancellationToken,
    ) -> io::Result<bool> {
        let command = self.diff_command(base, head, path, cancel).await?;
        process::read(command, cancel, async |stdout| {
            let mut reader = BufReader::new(stdout);
            while let Some(prefix) = process::line_prefix(&mut reader).await? {
                if let Some((start, count)) = hunk_range(&prefix, side) {
                    let line = u64::from(line);
                    if start <= line && line - start < count {
                        return Ok((true, true));
                    }
                }
            }
            Ok((false, false))
        })
        .await
    }

    pub(crate) async fn matches(
        &self,
        commit: &str,
        path: &str,
        line: u32,
        quote: &str,
        cancel: &CancellationToken,
    ) -> io::Result<bool> {
        if line == 0 || quote.is_empty() || !self.contains(commit, path, cancel).await? {
            return Ok(false);
        }
        let command = self.show_command(commit, path)?;
        process::read(command, cancel, async |stdout| {
            let mut reader = BufReader::new(stdout);
            for _ in 1..line {
                if !process::skip_line(&mut reader).await? {
                    return Ok((false, false));
                }
            }
            for expected in quote.lines() {
                if !process::matches_line(&mut reader, expected.as_bytes()).await? {
                    return Ok((false, true));
                }
            }
            Ok((true, true))
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

fn hunk_range(prefix: &[u8], side: GitSide) -> Option<(u64, u64)> {
    let line = std::str::from_utf8(prefix).ok()?.strip_prefix("@@ ")?;
    let value = line
        .split_whitespace()
        .nth(usize::from(side == GitSide::Head))?;
    let value = value.strip_prefix(if side == GitSide::Base { '-' } else { '+' })?;
    let (start, count) = value.split_once(',').unwrap_or((value, "1"));
    Some((start.parse().ok()?, count.parse().ok()?))
}

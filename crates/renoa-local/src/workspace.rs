use std::{
    io,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use renoa_agent::{Tool, ToolError};
use renoa_agent_loop::AgentToolBinding;
use renoa_harness::{ToolBinding, ToolRecovery};
use renoa_kernel::EffectRecovery;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    bash::Bash,
    file_tools::{EditFile, ReadFile, WriteFile},
    ripgrep::Ripgrep,
    search::{Find, Grep},
};

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LocalWorkspaceError {
    #[error("workspace path is not a directory: {0}")]
    NotDirectory(PathBuf),
    #[error("workspace path cannot be resolved: {0}")]
    Unavailable(#[source] io::Error),
    #[error("ripgrep (`rg`) is required but was not found on PATH")]
    RipgrepUnavailable,
    #[error("ripgrep cannot be inspected: {0}")]
    RipgrepInspection(#[source] io::Error),
    #[error("the resolved `rg` executable did not report a valid ripgrep version")]
    InvalidRipgrepVersion,
}

/// A canonical local directory exposed through a small coding-tool set.
pub struct LocalWorkspace {
    root: Arc<PathBuf>,
    binding_id: String,
    ripgrep: Arc<Ripgrep>,
}

impl LocalWorkspace {
    /// Opens one existing directory as the fixed root for every tool call.
    ///
    /// # Errors
    ///
    /// Returns an error when the path cannot be canonicalized or is not a directory.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, LocalWorkspaceError> {
        let root = std::fs::canonicalize(root).map_err(LocalWorkspaceError::Unavailable)?;
        if !root.is_dir() {
            return Err(LocalWorkspaceError::NotDirectory(root));
        }
        let ripgrep = Arc::new(Ripgrep::discover()?);
        Ok(Self {
            binding_id: hex_sha256(root.as_os_str().as_encoded_bytes()),
            root: Arc::new(root),
            ripgrep,
        })
    }

    /// Stable identity of the canonical root frozen into runtime bindings.
    #[must_use]
    pub(crate) fn binding_id(&self) -> &str {
        &self.binding_id
    }

    /// Creates the concrete tool bindings for one runtime profile.
    #[must_use]
    pub fn tool_bindings(&self) -> Vec<ToolBinding> {
        self.tools()
            .into_iter()
            .map(|binding| {
                ToolBinding::new(
                    binding.id,
                    binding.tool,
                    match binding.recovery {
                        LocalRecovery::SafeToReplay => ToolRecovery::SafeToReplay,
                        LocalRecovery::NeverReplay => ToolRecovery::NeverReplay,
                    },
                )
            })
            .collect()
    }

    /// Creates the same concrete tools for the decision-only kernel agent loop.
    #[must_use]
    pub fn kernel_tool_bindings(&self) -> Vec<AgentToolBinding> {
        self.tools()
            .into_iter()
            .map(|binding| {
                AgentToolBinding::new(
                    binding.id,
                    binding.tool,
                    match binding.recovery {
                        LocalRecovery::SafeToReplay => EffectRecovery::SafeToReplay,
                        LocalRecovery::NeverReplay => EffectRecovery::NeverReplay,
                    },
                )
            })
            .collect()
    }

    fn tool_binding_id(&self, tool: &str) -> String {
        format!("renoa-local/{tool}/{}", self.binding_id)
    }

    fn tools(&self) -> Vec<LocalToolBinding> {
        vec![
            LocalToolBinding {
                id: self.tool_binding_id("read-file-v2"),
                tool: Arc::new(ReadFile::new(Arc::clone(&self.root))),
                recovery: LocalRecovery::SafeToReplay,
            },
            LocalToolBinding {
                id: self.tool_binding_id("edit-file-v2"),
                tool: Arc::new(EditFile::new(Arc::clone(&self.root))),
                recovery: LocalRecovery::NeverReplay,
            },
            LocalToolBinding {
                id: self.tool_binding_id("write-file-v2"),
                tool: Arc::new(WriteFile::new(Arc::clone(&self.root))),
                recovery: LocalRecovery::NeverReplay,
            },
            LocalToolBinding {
                id: self.tool_binding_id("bash-v2"),
                tool: Arc::new(Bash::new(Arc::clone(&self.root))),
                recovery: LocalRecovery::NeverReplay,
            },
            LocalToolBinding {
                id: format!(
                    "renoa-local/grep-v2/{}/rg-{}",
                    self.binding_id,
                    self.ripgrep.revision()
                ),
                tool: Arc::new(Grep::new(Arc::clone(&self.root), Arc::clone(&self.ripgrep))),
                recovery: LocalRecovery::SafeToReplay,
            },
            LocalToolBinding {
                id: format!(
                    "renoa-local/find-v2/{}/rg-{}",
                    self.binding_id,
                    self.ripgrep.revision()
                ),
                tool: Arc::new(Find::new(Arc::clone(&self.root), Arc::clone(&self.ripgrep))),
                recovery: LocalRecovery::SafeToReplay,
            },
        ]
    }
}

struct LocalToolBinding {
    id: String,
    tool: Arc<dyn Tool>,
    recovery: LocalRecovery,
}

#[derive(Clone, Copy)]
enum LocalRecovery {
    SafeToReplay,
    NeverReplay,
}

pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to String cannot fail");
            output
        })
}

pub(crate) async fn existing_file(root: &Path, requested: &str) -> Result<PathBuf, ToolError> {
    let path = existing_path(root, requested).await?;
    if !path.is_file() {
        return Err(ToolError::new("path must name a file"));
    }
    Ok(path)
}

pub(crate) async fn existing_path(root: &Path, requested: &str) -> Result<PathBuf, ToolError> {
    let requested = relative_path(requested)?;
    let path = tokio::fs::canonicalize(root.join(requested))
        .await
        .map_err(|error| tool_error("resolve path", error))?;
    if !path.starts_with(root) {
        return Err(ToolError::new("path escapes the workspace"));
    }
    Ok(path)
}

pub(crate) async fn existing_directory(root: &Path, requested: &str) -> Result<PathBuf, ToolError> {
    let path = existing_path(root, requested).await?;
    if !path.is_dir() {
        return Err(ToolError::new("path must name a directory"));
    }
    Ok(path)
}

pub(crate) fn ensure_visible_search_path(
    root: &Path,
    requested: &str,
    resolved: &Path,
) -> Result<(), ToolError> {
    let resolved = resolved
        .strip_prefix(root)
        .map_err(|_| ToolError::new("search path escapes the workspace"))?;
    if has_hidden_component(Path::new(requested)) || has_hidden_component(resolved) {
        return Err(ToolError::new(
            "grep and find skip hidden paths; use bash for explicit hidden-file access",
        ));
    }
    Ok(())
}

fn has_hidden_component(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(component, Component::Normal(name) if name.as_encoded_bytes().starts_with(b"."))
    })
}

pub(crate) async fn writable_path(root: &Path, requested: &str) -> Result<PathBuf, ToolError> {
    let requested = relative_path(requested)?;
    let candidate = root.join(requested);
    match tokio::fs::symlink_metadata(&candidate).await {
        Ok(_) => return existing_path(root, requested.to_string_lossy().as_ref()).await,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(tool_error("inspect path", error)),
    }
    let parent = candidate
        .parent()
        .ok_or_else(|| ToolError::new("file path has no parent directory"))?;
    let parent = tokio::fs::canonicalize(parent)
        .await
        .map_err(|error| tool_error("resolve parent directory", error))?;
    if !parent.starts_with(root) {
        return Err(ToolError::new("path escapes the workspace"));
    }
    let name = candidate
        .file_name()
        .ok_or_else(|| ToolError::new("path must name a file"))?;
    Ok(parent.join(name))
}

pub(crate) fn relative_path(requested: &str) -> Result<&Path, ToolError> {
    let path = Path::new(requested);
    if requested.is_empty()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::Prefix(_)
                    | std::path::Component::RootDir
                    | std::path::Component::ParentDir
            )
        })
    {
        return Err(ToolError::new("path must stay relative to the workspace"));
    }
    Ok(path)
}

fn tool_error(action: &str, error: impl std::fmt::Display) -> ToolError {
    ToolError::new(format!("cannot {action}: {error}"))
}

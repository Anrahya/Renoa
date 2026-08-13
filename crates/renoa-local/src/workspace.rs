use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolExecutionMode, ToolOutput, ToolSpec,
    ToolUpdates,
};
use renoa_harness::{ToolBinding, ToolRecovery};
use serde_json::{Value, json};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::bash::Bash;

const OUTPUT_LIMIT: usize = 1_000_000;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LocalWorkspaceError {
    #[error("workspace path is not a directory: {0}")]
    NotDirectory(PathBuf),
    #[error("workspace path cannot be resolved: {0}")]
    Unavailable(#[source] io::Error),
}

/// A canonical local directory exposed through a small coding-tool set.
pub struct LocalWorkspace {
    root: Arc<PathBuf>,
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
        Ok(Self {
            root: Arc::new(root),
        })
    }

    /// Creates the concrete tool bindings for one runtime profile.
    #[must_use]
    pub fn tool_bindings(&self) -> Vec<ToolBinding> {
        vec![
            ToolBinding::new(
                "renoa-local/read-file-v1",
                Arc::new(ReadFile::new(Arc::clone(&self.root))),
                ToolRecovery::SafeToReplay,
            ),
            ToolBinding::new(
                "renoa-local/edit-file-v1",
                Arc::new(EditFile::new(Arc::clone(&self.root))),
                ToolRecovery::NeverReplay,
            ),
            ToolBinding::new(
                "renoa-local/write-file-v1",
                Arc::new(WriteFile::new(Arc::clone(&self.root))),
                ToolRecovery::NeverReplay,
            ),
            ToolBinding::new(
                "renoa-local/bash-v1",
                Arc::new(Bash::new(Arc::clone(&self.root))),
                ToolRecovery::NeverReplay,
            ),
        ]
    }
}

struct WriteFile {
    root: Arc<PathBuf>,
    spec: ToolSpec,
}

impl WriteFile {
    fn new(root: Arc<PathBuf>) -> Self {
        Self {
            root,
            spec: ToolSpec {
                name: "write_file".to_owned(),
                description: "Create or replace one UTF-8 text file inside the workspace."
                    .to_owned(),
                input_schema: object_schema(
                    &["path", "content"],
                    &json!({
                        "path": { "type": "string" },
                        "content": { "type": "string" }
                    }),
                ),
            },
        }
    }
}

impl Tool for WriteFile {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Sequential
    }

    fn execute(
        &self,
        call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let path = writable_path(&self.root, string_argument(&call.arguments, "path")?).await?;
            let content = string_argument(&call.arguments, "content")?;
            if content.len() > OUTPUT_LIMIT {
                return Err(ToolError::new(format!(
                    "content exceeds the {OUTPUT_LIMIT}-byte write limit"
                )));
            }
            tokio::fs::write(&path, content)
                .await
                .map_err(|error| tool_error("write file", error))?;
            Ok(ToolOutput {
                content: vec![ContentBlock::text(format!("Wrote {}", path.display()))],
                details: Some(json!({ "path": path })),
            })
        })
    }
}

struct ReadFile {
    root: Arc<PathBuf>,
    spec: ToolSpec,
}

impl ReadFile {
    fn new(root: Arc<PathBuf>) -> Self {
        Self {
            root,
            spec: ToolSpec {
                name: "read_file".to_owned(),
                description: "Read one UTF-8 text file inside the workspace.".to_owned(),
                input_schema: object_schema(&["path"], &json!({ "path": { "type": "string" } })),
            },
        }
    }
}

impl Tool for ReadFile {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let path = existing_path(&self.root, string_argument(&call.arguments, "path")?).await?;
            let bytes = tokio::fs::read(&path)
                .await
                .map_err(|error| tool_error("read file", error))?;
            if bytes.len() > OUTPUT_LIMIT {
                return Err(ToolError::new(format!(
                    "file exceeds the {OUTPUT_LIMIT}-byte read limit"
                )));
            }
            let text = String::from_utf8(bytes)
                .map_err(|_| ToolError::new("file is not valid UTF-8 text"))?;
            Ok(ToolOutput {
                content: vec![ContentBlock::text(text)],
                details: Some(json!({ "path": path })),
            })
        })
    }
}

struct EditFile {
    root: Arc<PathBuf>,
    spec: ToolSpec,
}

impl EditFile {
    fn new(root: Arc<PathBuf>) -> Self {
        Self {
            root,
            spec: ToolSpec {
                name: "edit_file".to_owned(),
                description: "Replace one exact text occurrence in a workspace file.".to_owned(),
                input_schema: object_schema(
                    &["path", "old_text", "new_text"],
                    &json!({
                        "path": { "type": "string" },
                        "old_text": { "type": "string", "minLength": 1 },
                        "new_text": { "type": "string" }
                    }),
                ),
            },
        }
    }
}

impl Tool for EditFile {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Sequential
    }

    fn execute(
        &self,
        call: ToolCall,
        _cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let path = existing_path(&self.root, string_argument(&call.arguments, "path")?).await?;
            let old_text = string_argument(&call.arguments, "old_text")?;
            if old_text.is_empty() {
                return Err(ToolError::new("old_text must not be empty"));
            }
            let new_text = string_argument(&call.arguments, "new_text")?;
            let content = tokio::fs::read_to_string(&path)
                .await
                .map_err(|error| tool_error("read file", error))?;
            let Some(start) = content.find(old_text) else {
                return Err(ToolError::new("old_text was not found"));
            };
            if content[start + old_text.len()..].contains(old_text) {
                return Err(ToolError::new("old_text occurs more than once"));
            }
            let mut edited = String::with_capacity(content.len() - old_text.len() + new_text.len());
            edited.push_str(&content[..start]);
            edited.push_str(new_text);
            edited.push_str(&content[start + old_text.len()..]);
            tokio::fs::write(&path, edited)
                .await
                .map_err(|error| tool_error("write file", error))?;
            Ok(ToolOutput {
                content: vec![ContentBlock::text(format!("Edited {}", path.display()))],
                details: Some(json!({ "path": path })),
            })
        })
    }
}

async fn existing_path(root: &Path, requested: &str) -> Result<PathBuf, ToolError> {
    let requested = relative_path(requested)?;
    let path = tokio::fs::canonicalize(root.join(requested))
        .await
        .map_err(|error| tool_error("resolve path", error))?;
    if !path.starts_with(root) {
        return Err(ToolError::new("path escapes the workspace"));
    }
    Ok(path)
}

async fn writable_path(root: &Path, requested: &str) -> Result<PathBuf, ToolError> {
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

fn relative_path(requested: &str) -> Result<&Path, ToolError> {
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

fn string_argument<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, ToolError> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new(format!("{name} must be a string")))
}

fn object_schema(required: &[&str], properties: &Value) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn tool_error(action: &str, error: impl std::fmt::Display) -> ToolError {
    ToolError::new(format!("cannot {action}: {error}"))
}

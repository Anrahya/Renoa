//! `share_file` and `stop_sharing`: an agent hands the person it talks to a
//! file from its workspace as a link, and takes it back. The link is relative,
//! `/s/<share>`, so each surface opens it against the Renoa it signed in to.
//! A share is one row, so either call replayed converges on the same state.

use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::OwnerTurn;
use crate::{
    images::shown_size,
    shares::{self, MAX_SHARE_BYTES, SharedPath},
    tool_input::decode,
    workspace::{existing_file, relative_path},
};

pub(crate) const SHARE: &str = "share_file";
pub(crate) const STOP: &str = "stop_sharing";

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Share,
    Stop,
}

pub(super) fn spec(kind: Kind) -> ToolSpec {
    let path = json!({
        "type": "object",
        "properties": {"path": {"type": "string", "minLength": 1}},
        "required": ["path"],
        "additionalProperties": false
    });
    match kind {
        Kind::Share => ToolSpec {
            name: SHARE.to_owned(),
            description: format!(
                "Share a file from your workspace with the person you are talking to, such as a page, picture or document you made. Returns a link to put in your reply exactly as given; only they can open it, on their phone or signed in to Renoa in a browser. The link shows the file as it is when they open it, and stops working once the file is gone. A file up to {} can be shared. Do not start a server to show a file.",
                shown_size(usize::try_from(MAX_SHARE_BYTES).unwrap_or(usize::MAX))
            ),
            input_schema: path,
        },
        Kind::Stop => ToolSpec {
            name: STOP.to_owned(),
            description: "Stop sharing a file you shared with share_file; its link stops working."
                .to_owned(),
            input_schema: path,
        },
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    path: String,
}

pub(super) struct ShareTool {
    pub(super) kind: Kind,
    pub(super) turn: Arc<OwnerTurn>,
    pub(super) spec: ToolSpec,
}

impl Tool for ShareTool {
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
            let input: Input = decode(call.arguments)?;
            let workspace = tokio::fs::canonicalize(&self.turn.workspace)
                .await
                .map_err(|error| {
                    ToolError::unavailable(format!("your workspace cannot be read: {error}"))
                })?;
            match self.kind {
                Kind::Share => self.share(workspace, &input.path).await,
                Kind::Stop => self.stop(workspace, &input.path).await,
            }
        })
    }
}

impl ShareTool {
    async fn share(&self, workspace: PathBuf, requested: &str) -> Result<ToolOutput, ToolError> {
        let file = existing_file(&workspace, requested).await?;
        let size = tokio::fs::metadata(&file)
            .await
            .map_err(|error| {
                ToolError::unavailable(format!("{requested} cannot be read: {error}"))
            })?
            .len();
        if size > MAX_SHARE_BYTES {
            return Err(ToolError::invalid_input(format!(
                "{requested} is {}; a shared file is at most {}. Make a smaller copy and share that.",
                shown_size(usize::try_from(size).unwrap_or(usize::MAX)),
                shown_size(usize::try_from(MAX_SHARE_BYTES).unwrap_or(usize::MAX))
            )));
        }
        let path = inside(&workspace, &file)?;
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&path)
            .to_owned();
        let shared = SharedPath { workspace, path };
        let (turn, file) = (Arc::clone(&self.turn), shared.clone());
        let id =
            saved(move || shares::share(&turn.database, turn.agent, turn.principal, &file)).await?;
        Ok(ToolOutput {
            content: vec![ContentBlock::text(format!(
                "Shared {}. Put this link in your reply exactly as it is: [{name}](/s/{id})",
                shared.path
            ))],
            details: Some(json!({ "path": shared.path, "share_id": id })),
            is_error: false,
            closing: None,
        })
    }

    async fn stop(&self, workspace: PathBuf, requested: &str) -> Result<ToolOutput, ToolError> {
        let path = match existing_file(&workspace, requested).await {
            Ok(file) => inside(&workspace, &file)?,
            Err(_) => gone(&workspace, requested).await?,
        };
        let shared = SharedPath { workspace, path };
        let (turn, file) = (Arc::clone(&self.turn), shared.clone());
        let stopped =
            saved(move || shares::unshare(&turn.database, turn.agent, turn.principal, &file))
                .await?;
        let text = if stopped {
            format!("Stopped sharing {}; its link no longer opens.", shared.path)
        } else {
            format!("{} was not shared, so nothing changed.", shared.path)
        };
        Ok(ToolOutput {
            content: vec![ContentBlock::text(text)],
            details: Some(json!({ "path": shared.path, "stopped": stopped })),
            is_error: false,
            closing: None,
        })
    }
}

/// Runs one share change, which blocks on the catalog. A failure says nothing
/// changed: a share is one row, written in one statement.
async fn saved<T: Send + 'static>(
    change: impl FnOnce() -> Result<T, crate::host::catalog::HostCatalogError> + Send + 'static,
) -> Result<T, ToolError> {
    let outcome = tokio::task::spawn_blocking(change)
        .await
        .map_err(|error| error.to_string())
        .and_then(|outcome| outcome.map_err(|error| error.to_string()));
    outcome.map_err(|error| {
        renoa_telemetry::event("renoa.host", "warn", "share_unsaved", &json!({ "error": error }));
        ToolError::unavailable(
            "The share could not be saved, so nothing changed. Try once more; if it fails again, tell the person in your reply.",
        )
    })
}

/// Where a file already gone was, spelled as its share stores it: its folder
/// resolved as far as it still exists, then its own name.
async fn gone(workspace: &Path, requested: &str) -> Result<String, ToolError> {
    let mut parts: Vec<_> = relative_path(requested)?
        .components()
        .filter_map(|part| match part {
            Component::Normal(part) => Some(part),
            _ => None,
        })
        .collect();
    let name = parts
        .pop()
        .ok_or_else(|| ToolError::invalid_input("path must name a file"))?;
    let folder = workspace.join(parts.iter().collect::<PathBuf>());
    let folder = tokio::fs::canonicalize(&folder).await.unwrap_or(folder);
    inside(workspace, &folder.join(name))
}

/// `file`'s path inside `workspace`, as a share stores it.
fn inside(workspace: &Path, file: &Path) -> Result<String, ToolError> {
    file.strip_prefix(workspace)
        .map_err(|_| ToolError::permission_denied("path escapes the workspace"))?
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| ToolError::invalid_input("only a file whose path is UTF-8 can be shared"))
}

#[cfg(test)]
mod tests;

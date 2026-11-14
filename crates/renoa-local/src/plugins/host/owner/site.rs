//! `site_manage`: publish a static site folder from the agent's workspace as an
//! immutable snapshot the node's site gateway serves at `/sites/<name>/`, list
//! published sites, or unpublish one.

use std::sync::Arc;

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::OwnerTurn;
use crate::{
    sites::{SiteError, Sites},
    tool_input::decode,
};

pub(crate) const MANAGE: &str = "site_manage";

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Publish,
    List,
    Unpublish,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    action: Action,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    folder: Option<String>,
}

pub(super) struct SiteTool {
    pub(super) turn: Arc<OwnerTurn>,
    pub(super) spec: ToolSpec,
}

pub(super) fn spec() -> ToolSpec {
    ToolSpec {
        name: MANAGE.to_owned(),
        description: "Publish a static web site from a workspace folder as a link the owner opens in their browser, list published sites, or unpublish one. Publishing snapshots the folder so later workspace edits do not affect the live site until you publish again under the same name. The site is served under /sites/<name>/, so build it with relative asset paths (Vite: base './'). Actions: publish (requires name and folder), list, unpublish (requires name)."
            .to_owned(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["publish", "list", "unpublish"]
                },
                "name": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 63,
                    "description": "Site slug (1-63 lowercase letters, digits, or hyphens). Required for publish and unpublish."
                },
                "folder": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Workspace-relative folder containing the static site (for example \"dist\" or \"out\"). Required for publish."
                }
            },
            "required": ["action"],
            "additionalProperties": false
        }),
    }
}

impl Tool for SiteTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            if call.name != MANAGE {
                return Err(ToolError::invalid_input(format!(
                    "tool binding `{MANAGE}` cannot execute call for `{}`",
                    call.name
                )));
            }
            let input: Input = decode(call.arguments)?;
            if cancellation.is_cancelled() {
                return Err(ToolError::cancelled(
                    format!("{MANAGE} was cancelled before making changes"),
                    false,
                ));
            }
            let turn = Arc::clone(&self.turn);
            run_action(&turn, input).await
        })
    }
}

async fn run_action(turn: &OwnerTurn, input: Input) -> Result<ToolOutput, ToolError> {
    let Some(origin) = turn.site_origin.as_deref() else {
        return Err(ToolError::unavailable(
            "Static sites are not set up on this Renoa: the owner has not turned on the site gateway. Tell the owner, and share single files with share_file instead.",
        ));
    };
    let sites = Sites::from_database(&turn.database);
    match input.action {
        Action::Publish => {
            let (Some(name), Some(folder)) = (
                non_empty_trimmed(input.name),
                non_empty_trimmed(input.folder),
            ) else {
                return Err(ToolError::invalid_input(
                    r#"site_manage action "publish" requires "name" (1-63 lowercase letters, digits, or hyphens) and "folder" (workspace-relative path). Example: {"action": "publish", "name": "demo", "folder": "dist"}"#,
                ));
            };
            let published = sites
                .publish(
                    turn.agent,
                    turn.principal,
                    turn.workspace.clone(),
                    name,
                    folder,
                )
                .await
                .map_err(|error| map_site_error(&error))?;
            let noun = if published.file_count == 1 {
                "file"
            } else {
                "files"
            };
            let link = format!("[{}]({origin}{})", published.name, published.path);
            Ok(ToolOutput::text(format!(
                "Published {link} ({} {noun}, {} bytes). Write {link} in your reply so the owner can open it.",
                published.file_count, published.total_bytes
            )))
        }
        Action::List => {
            if input.name.is_some() || input.folder.is_some() {
                return Err(ToolError::invalid_input(
                    r#"site_manage action "list" takes no "name" or "folder". Example: {"action": "list"}"#,
                ));
            }
            let listed = sites
                .list_for(turn.principal)
                .await
                .map_err(|error| map_site_error(&error))?;
            if listed.is_empty() {
                return Ok(ToolOutput::text("No static sites are published."));
            }
            let lines = listed
                .into_iter()
                .map(|site| {
                    format!(
                        "- [{}]({origin}{}) (source: {}, {} files, {} bytes)",
                        site.name, site.path, site.source_folder, site.file_count, site.total_bytes
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            Ok(ToolOutput::text(lines))
        }
        Action::Unpublish => {
            if input.folder.is_some() {
                return Err(ToolError::invalid_input(
                    r#"site_manage action "unpublish" takes "name" only, not "folder". Example: {"action": "unpublish", "name": "demo"}"#,
                ));
            }
            let Some(name) = non_empty_trimmed(input.name) else {
                return Err(ToolError::invalid_input(
                    r#"site_manage action "unpublish" requires "name". Example: {"action": "unpublish", "name": "demo"}"#,
                ));
            };
            let removed = sites
                .unpublish_for(turn.principal, &name)
                .await
                .map_err(|error| map_site_error(&error))?;
            if removed {
                Ok(ToolOutput::text("Unpublished the site."))
            } else {
                Ok(ToolOutput::text(
                    "No published site with that name was found.",
                ))
            }
        }
    }
}

fn non_empty_trimmed(value: Option<String>) -> Option<String> {
    value.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

fn map_site_error(error: &SiteError) -> ToolError {
    match error {
        SiteError::InvalidName
        | SiteError::InvalidFolder(_)
        | SiteError::TooManyEntries
        | SiteError::SiteTooLarge
        | SiteError::NotFound => ToolError::invalid_input(error.to_string()),
        SiteError::PermissionDenied => ToolError::permission_denied(error.to_string()),
        SiteError::Catalog(_) | SiteError::Io(_) | SiteError::Background(_) => {
            renoa_telemetry::event(
                "renoa.host",
                "warn",
                "site_manage_failed",
                &json!({ "error": error.to_string() }),
            );
            ToolError::unavailable(
                "The static site could not be stored just now. Retry in a moment.",
            )
        }
    }
}

trait ToolOutputText {
    fn text(text: impl Into<String>) -> Self;
}

impl ToolOutputText for ToolOutput {
    fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::text(text.into())],
            details: None,
            is_error: false,
            closing: None,
        }
    }
}

#[cfg(test)]
mod tests;

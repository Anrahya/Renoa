use std::{path::PathBuf, sync::Arc};

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::GitRepository;

#[derive(Clone, Copy)]
pub(crate) enum Operation {
    Changes,
    Diff,
    Show,
}

pub(crate) struct GitTool {
    root: Arc<PathBuf>,
    operation: Operation,
    spec: ToolSpec,
}

impl GitTool {
    pub(crate) fn new(root: Arc<PathBuf>, operation: Operation) -> Self {
        let (name, description, fields, required) = match operation {
            Operation::Changes => (
                "git_changes",
                "List every changed path between immutable commits, including deletions, renames, binary and hidden files. Follow next_offset until null; the page size never limits the change inventory.",
                json!({"base":{"type":"string"},"head":{"type":"string"},"offset":{"type":"integer","minimum":0}}),
                vec!["base", "head"],
            ),
            Operation::Diff => (
                "git_diff",
                "Read a unified diff for one repository-relative path between immutable commits. offset is a byte cursor; follow next_offset with identical arguments to read all pages. For a rename, inspect both paths. No external diff or text conversion programs run.",
                json!({"base":{"type":"string"},"head":{"type":"string"},"path":{"type":"string"},"offset":{"type":"integer","minimum":0}}),
                vec!["base", "head", "path"],
            ),
            Operation::Show => (
                "git_show",
                "Read a tracked blob at an immutable commit, including base instructions, deleted files and hidden paths. Follow the returned byte cursor until next_offset is null. Non-UTF-8 pages are returned losslessly as base64. This reads Git objects, never symlink targets.",
                json!({"commit":{"type":"string"},"path":{"type":"string"},"offset":{"type":"integer","minimum":0}}),
                vec!["commit", "path"],
            ),
        };
        Self {
            root,
            operation,
            spec: ToolSpec {
                name: name.to_owned(),
                description: format!(
                    "{description} Commit IDs must be full SHA-1 or SHA-256 hex IDs. The workspace root must be a Git repository or registered linked worktree."
                ),
                input_schema: json!({"type":"object","additionalProperties":false,"properties":fields,"required":required}),
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Changes {
    base: String,
    head: String,
    #[serde(default)]
    offset: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Diff {
    base: String,
    head: String,
    path: String,
    #[serde(default)]
    offset: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Show {
    commit: String,
    path: String,
    #[serde(default)]
    offset: u64,
}

impl Tool for GitTool {
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
            let root = Arc::clone(&self.root);
            let repository = tokio::task::spawn_blocking(move || GitRepository::open(&root))
                .await
                .map_err(|error| ToolError::internal(error.to_string()))?
                .map_err(|error| {
                    crate::tool_error::io_error("open Git repository", &error, false)
                })?;
            let output = match self.operation {
                Operation::Changes => {
                    let input: Changes = crate::tool_input::decode(call.arguments)?;
                    let changes = repository
                        .changes(&input.base, &input.head, &cancellation)
                        .await;
                    let changes = changes.map_err(|error| {
                        crate::tool_error::io_error("list Git changes", &error, false)
                    })?;
                    if input.offset > changes.len() {
                        return Err(ToolError::invalid_input("offset exceeds change inventory"));
                    }
                    let mut end = input.offset;
                    let mut bytes = 0;
                    while let Some(change) = changes.get(end) {
                        let size = serde_json::to_vec(change)
                            .map_err(|error| ToolError::internal(error.to_string()))?
                            .len();
                        if end > input.offset && bytes + size > crate::output::MAX_TOOL_OUTPUT_BYTES
                        {
                            break;
                        }
                        bytes += size;
                        end += 1;
                    }
                    json!({"base":input.base,"head":input.head,"total":changes.len(),"offset":input.offset,
                        "next_offset":(end < changes.len()).then_some(end),"changes":&changes[input.offset..end]})
                }
                Operation::Diff => {
                    let input: Diff = crate::tool_input::decode(call.arguments)?;
                    let page = repository
                        .diff(
                            &input.base,
                            &input.head,
                            &input.path,
                            input.offset,
                            &cancellation,
                        )
                        .await
                        .map_err(|error| {
                            crate::tool_error::io_error("read Git diff", &error, false)
                        })?;
                    json!({"base":input.base,"head":input.head,"path":input.path,"page":page})
                }
                Operation::Show => {
                    let input: Show = crate::tool_input::decode(call.arguments)?;
                    let exists = repository
                        .contains(&input.commit, &input.path, &cancellation)
                        .await
                        .map_err(|error| {
                            crate::tool_error::io_error("locate Git blob", &error, false)
                        })?;
                    if exists {
                        let page = repository
                            .show(&input.commit, &input.path, input.offset, &cancellation)
                            .await
                            .map_err(|error| {
                                crate::tool_error::io_error("read Git blob", &error, false)
                            })?;
                        json!({"commit":input.commit,"path":input.path,"exists":true,"page":page})
                    } else {
                        json!({"commit":input.commit,"path":input.path,"exists":false})
                    }
                }
            };
            Ok(ToolOutput {
                content: vec![ContentBlock::text(output.to_string())],
                details: None,
                is_error: false,
            })
        })
    }
}

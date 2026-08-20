use std::{path::PathBuf, sync::Arc};

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolExecutionMode, ToolOutput, ToolSpec,
    ToolUpdates,
};
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

use crate::{
    output::{HeadOutput, MAX_TOOL_OUTPUT_BYTES, MAX_TOOL_OUTPUT_LINES, truncation_notice},
    tool_input::{bounded_limit, decode, non_empty},
    workspace::{existing_file, writable_path},
};

const MAX_FILE_WRITE_BYTES: usize = 1_000_000;

pub(crate) struct WriteFile {
    root: Arc<PathBuf>,
    spec: ToolSpec,
}

impl WriteFile {
    pub(crate) fn new(root: Arc<PathBuf>) -> Self {
        Self {
            root,
            spec: ToolSpec {
                name: "write_file".to_owned(),
                description: "Create or replace one UTF-8 text file inside the workspace."
                    .to_owned(),
                input_schema: object_schema(
                    &["path", "content"],
                    &json!({
                        "path": { "type": "string", "minLength": 1 },
                        "content": { "type": "string" }
                    }),
                ),
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteFileInput {
    path: String,
    content: String,
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
            let input: WriteFileInput = decode(call.arguments)?;
            let path = writable_path(&self.root, &input.path).await?;
            if input.content.len() > MAX_FILE_WRITE_BYTES {
                return Err(ToolError::new(format!(
                    "content exceeds the {MAX_FILE_WRITE_BYTES}-byte write limit"
                )));
            }
            tokio::fs::write(&path, input.content)
                .await
                .map_err(|error| tool_error("write file", error))?;
            Ok(ToolOutput {
                content: vec![ContentBlock::text(format!("Wrote {}", input.path))],
                details: Some(json!({ "path": input.path })),
            })
        })
    }
}

pub(crate) struct ReadFile {
    root: Arc<PathBuf>,
    spec: ToolSpec,
}

impl ReadFile {
    pub(crate) fn new(root: Arc<PathBuf>) -> Self {
        Self {
            root,
            spec: ToolSpec {
                name: "read_file".to_owned(),
                description: concat!(
                    "Read UTF-8 text from a workspace file. Line offsets are 1-based. ",
                    "Output is capped at 2,000 lines or 50 KiB; continue with the returned offset."
                )
                .to_owned(),
                input_schema: object_schema(
                    &["path"],
                    &json!({
                        "path": { "type": "string", "minLength": 1 },
                        "offset": { "type": "integer", "minimum": 1 },
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": MAX_TOOL_OUTPUT_LINES
                        }
                    }),
                ),
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadFileInput {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

impl Tool for ReadFile {
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
            let input: ReadFileInput = decode(call.arguments)?;
            let offset = input.offset.unwrap_or(1);
            if offset == 0 {
                return Err(ToolError::new("offset must be at least 1"));
            }
            let limit = bounded_limit(input.limit, MAX_TOOL_OUTPUT_LINES)?;
            let path = existing_file(&self.root, &input.path).await?;
            let page = read_page(&path, offset, limit, &cancellation).await?;
            Ok(ToolOutput {
                content: vec![ContentBlock::text(page.text)],
                details: Some(json!({
                    "path": path,
                    "start_line": offset,
                    "lines": page.lines,
                    "next_offset": page.next_offset,
                    "truncated": page.next_offset.is_some()
                })),
            })
        })
    }
}

struct ReadPage {
    text: String,
    lines: usize,
    next_offset: Option<usize>,
}

async fn read_page(
    path: &PathBuf,
    offset: usize,
    limit: usize,
    cancellation: &CancellationToken,
) -> Result<ReadPage, ToolError> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| tool_error("open file", error))?;
    let mut reader = BufReader::new(file);
    let mut line_number = 1;
    while line_number < offset {
        match read_bounded_line(&mut reader, false, 0, cancellation).await? {
            LineRead::End => {
                return Err(ToolError::new(format!(
                    "offset {offset} is beyond the end of the file"
                )));
            }
            LineRead::Complete(_) | LineRead::TooLong => line_number += 1,
        }
    }

    let mut output = HeadOutput::new();
    let mut oversized_line = false;
    while output.line_count() < limit {
        match read_bounded_line(&mut reader, true, output.remaining_bytes(), cancellation).await? {
            LineRead::End => break,
            LineRead::Complete(bytes) => {
                let line = String::from_utf8(bytes)
                    .map_err(|_| ToolError::new("file is not valid UTF-8 text"))?;
                if !output.push_line(&line) {
                    break;
                }
                line_number += 1;
            }
            LineRead::TooLong => {
                oversized_line = true;
                break;
            }
        }
    }

    let has_more = if oversized_line {
        true
    } else if output.line_count() == limit {
        !matches!(
            read_bounded_line(&mut reader, false, 0, cancellation).await?,
            LineRead::End
        )
    } else {
        false
    };
    if offset > 1 && output.line_count() == 0 && !has_more {
        return Err(ToolError::new(format!(
            "offset {offset} is beyond the end of the file"
        )));
    }

    let next_offset = has_more.then_some(line_number);
    let notice = if oversized_line && output.line_count() == 0 {
        Some(format!(
            "[Line {line_number} exceeds the {MAX_TOOL_OUTPUT_BYTES}-byte output limit. Use bash to inspect a byte slice.]"
        ))
    } else {
        next_offset.map(|next| truncation_notice("Read", next))
    };
    let lines = output.line_count();
    Ok(ReadPage {
        text: output.finish(notice.as_deref()),
        lines,
        next_offset,
    })
}

enum LineRead {
    End,
    Complete(Vec<u8>),
    TooLong,
}

async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    capture: bool,
    max_bytes: usize,
    cancellation: &CancellationToken,
) -> Result<LineRead, ToolError> {
    let mut line = Vec::new();
    let mut too_long = false;
    loop {
        let available = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                return Err(ToolError::new("read execution was cancelled"));
            }
            available = reader.fill_buf() => {
                available.map_err(|error| tool_error("read file", error))?
            }
        };
        if available.is_empty() {
            return if too_long {
                Ok(LineRead::TooLong)
            } else if line.is_empty() {
                Ok(LineRead::End)
            } else {
                Ok(LineRead::Complete(line))
            };
        }

        let consumed = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        let complete = available[consumed - 1] == b'\n';
        if capture && !too_long {
            if consumed <= max_bytes.saturating_sub(line.len()) {
                line.extend_from_slice(&available[..consumed]);
            } else {
                line.clear();
                too_long = true;
            }
        }
        reader.consume(consumed);
        if complete {
            return if too_long {
                Ok(LineRead::TooLong)
            } else {
                Ok(LineRead::Complete(line))
            };
        }
    }
}

pub(crate) struct EditFile {
    root: Arc<PathBuf>,
    spec: ToolSpec,
}

impl EditFile {
    pub(crate) fn new(root: Arc<PathBuf>) -> Self {
        Self {
            root,
            spec: ToolSpec {
                name: "edit_file".to_owned(),
                description: "Replace one exact text occurrence in a workspace file.".to_owned(),
                input_schema: object_schema(
                    &["path", "old_text", "new_text"],
                    &json!({
                        "path": { "type": "string", "minLength": 1 },
                        "old_text": { "type": "string", "minLength": 1 },
                        "new_text": { "type": "string" }
                    }),
                ),
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditFileInput {
    path: String,
    old_text: String,
    new_text: String,
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
            let input: EditFileInput = decode(call.arguments)?;
            non_empty("old_text", &input.old_text)?;
            let path = existing_file(&self.root, &input.path).await?;
            let content = tokio::fs::read_to_string(&path)
                .await
                .map_err(|error| tool_error("read file", error))?;
            let Some(start) = content.find(&input.old_text) else {
                return Err(ToolError::new("old_text was not found"));
            };
            if content[start + input.old_text.len()..].contains(&input.old_text) {
                return Err(ToolError::new("old_text occurs more than once"));
            }
            let edited_len = content
                .len()
                .saturating_sub(input.old_text.len())
                .saturating_add(input.new_text.len());
            if edited_len > MAX_FILE_WRITE_BYTES {
                return Err(ToolError::new(format!(
                    "edited file exceeds the {MAX_FILE_WRITE_BYTES}-byte write limit"
                )));
            }
            let mut edited = String::with_capacity(edited_len);
            edited.push_str(&content[..start]);
            edited.push_str(&input.new_text);
            edited.push_str(&content[start + input.old_text.len()..]);
            tokio::fs::write(&path, edited)
                .await
                .map_err(|error| tool_error("write file", error))?;
            Ok(ToolOutput {
                content: vec![ContentBlock::text(format!("Edited {}", input.path))],
                details: Some(json!({ "path": input.path })),
            })
        })
    }
}

fn object_schema(required: &[&str], properties: &serde_json::Value) -> serde_json::Value {
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

#[cfg(test)]
#[path = "file_tools_tests.rs"]
mod tests;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use globset::{GlobBuilder, GlobMatcher};
use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolOutput, ToolSpec, ToolUpdates,
};
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

use crate::{
    output::HeadOutput,
    ripgrep::{Ripgrep, SearchProcess},
    tool_error::io_error,
    tool_input::{bounded_limit, decode, non_empty},
    workspace::{ensure_visible_search_path, existing_directory, existing_path},
};

const GREP_MATCH_LIMIT: usize = 100;
const FIND_RESULT_LIMIT: usize = 1_000;
const GREP_LINE_COLUMNS: &str = "500";

pub(crate) struct Grep {
    root: Arc<PathBuf>,
    ripgrep: Arc<Ripgrep>,
    spec: ToolSpec,
}

impl Grep {
    pub(crate) fn new(root: Arc<PathBuf>, ripgrep: Arc<Ripgrep>) -> Self {
        Self {
            root,
            ripgrep,
            spec: ToolSpec {
                name: "grep".to_owned(),
                description: concat!(
                    "Search workspace text with a Rust regular expression. Returns relative paths, ",
                    "line numbers, and matching lines; respects ignore files and skips hidden paths. ",
                    "Use bash for hidden files."
                )
                .to_owned(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "minLength": 1 },
                        "path": { "type": "string", "minLength": 1 },
                        "glob": { "type": "string", "minLength": 1 },
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": GREP_MATCH_LIMIT
                        }
                    },
                    "required": ["pattern"],
                    "additionalProperties": false
                }),
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrepInput {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
    limit: Option<usize>,
}

impl Tool for Grep {
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
            let input: GrepInput = decode(call.arguments)?;
            non_empty("pattern", &input.pattern)?;
            if let Some(glob) = input.glob.as_deref() {
                non_empty("glob", glob)?;
            }
            let limit = bounded_limit(input.limit, GREP_MATCH_LIMIT)?;
            let requested_path = input.path.as_deref().unwrap_or(".");
            let search_path = existing_path(&self.root, requested_path).await?;
            ensure_visible_search_path(&self.root, requested_path, &search_path)?;
            let output = grep(
                &self.root,
                &self.ripgrep,
                &search_path,
                &input.pattern,
                input.glob.as_deref(),
                limit,
                &cancellation,
            )
            .await?;
            Ok(ToolOutput {
                content: vec![ContentBlock::text(output.text)],
                details: Some(json!({
                    "matches": output.items,
                    "truncated": output.truncated
                })),
            })
        })
    }
}

pub(crate) struct Find {
    root: Arc<PathBuf>,
    ripgrep: Arc<Ripgrep>,
    spec: ToolSpec,
}

impl Find {
    pub(crate) fn new(root: Arc<PathBuf>, ripgrep: Arc<Ripgrep>) -> Self {
        Self {
            root,
            ripgrep,
            spec: ToolSpec {
                name: "find".to_owned(),
                description: concat!(
                    "Find workspace files by glob pattern. Returns sorted relative paths; ",
                    "respects ignore files and skips hidden paths. Use bash for hidden files."
                )
                .to_owned(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "minLength": 1 },
                        "path": { "type": "string", "minLength": 1 },
                        "limit": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": FIND_RESULT_LIMIT
                        }
                    },
                    "required": ["pattern"],
                    "additionalProperties": false
                }),
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FindInput {
    pattern: String,
    path: Option<String>,
    limit: Option<usize>,
}

impl Tool for Find {
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
            let input: FindInput = decode(call.arguments)?;
            non_empty("pattern", &input.pattern)?;
            let limit = bounded_limit(input.limit, FIND_RESULT_LIMIT)?;
            let requested_path = input.path.as_deref().unwrap_or(".");
            let search_path = existing_directory(&self.root, requested_path).await?;
            ensure_visible_search_path(&self.root, requested_path, &search_path)?;
            let output = find(
                &self.root,
                &self.ripgrep,
                &search_path,
                &input.pattern,
                limit,
                &cancellation,
            )
            .await?;
            Ok(ToolOutput {
                content: vec![ContentBlock::text(output.text)],
                details: Some(json!({
                    "results": output.items,
                    "truncated": output.truncated
                })),
            })
        })
    }
}

struct SearchOutput {
    text: String,
    items: usize,
    truncated: bool,
}

async fn grep(
    root: &Path,
    ripgrep: &Ripgrep,
    search_path: &Path,
    pattern: &str,
    glob: Option<&str>,
    limit: usize,
    cancellation: &CancellationToken,
) -> Result<SearchOutput, ToolError> {
    let glob = glob.map(compile_glob).transpose()?;
    let mut command = ripgrep.command(root);
    command.args([
        "--json",
        "--line-number",
        "--max-columns",
        GREP_LINE_COLUMNS,
        "--max-columns-preview",
    ]);
    command.arg("--").arg(pattern).arg(search_path);
    let mut process = SearchProcess::start(command, "ripgrep")?;
    let mut reader = BufReader::new(process.take_stdout("ripgrep")?);
    let mut output = HeadOutput::new();
    let mut buffer = Vec::new();
    let mut truncated = false;
    loop {
        buffer.clear();
        let read = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                process.stop().await?;
                return Err(ToolError::cancelled("grep execution was cancelled", false));
            }
            read = reader.read_until(b'\n', &mut buffer) => read,
        };
        let read = match read {
            Ok(read) => read,
            Err(error) => {
                return stop_with_error(&mut process, tool_error("read ripgrep output", &error))
                    .await;
            }
        };
        if read == 0 {
            break;
        }
        let message: RipgrepMessage = match serde_json::from_slice(&buffer) {
            Ok(message) => message,
            Err(error) => {
                return stop_with_error(
                    &mut process,
                    ToolError::internal(format!("invalid ripgrep output: {error}")),
                )
                .await;
            }
        };
        let RipgrepMessage::Match { data } = message else {
            continue;
        };
        let rendered = match render_match(root, data, glob.as_ref()) {
            Ok(Some(rendered)) => rendered,
            Ok(None) => continue,
            Err(error) => return stop_with_error(&mut process, error).await,
        };
        if output.line_count() == limit {
            truncated = true;
            break;
        }
        if !output.push_line(&rendered) {
            truncated = true;
            break;
        }
    }

    let status = if truncated {
        process.stop().await?;
        None
    } else {
        Some(process.finish(cancellation, "grep").await?)
    };
    if let Some(status) = status {
        process.validate_status(status, true)?;
    }
    let items = output.line_count();
    let notice =
        truncated.then_some("[Grep output capped. Narrow the pattern, path, or glob to continue.]");
    let text = if items == 0 && !truncated {
        "No matches found.".to_owned()
    } else {
        output.finish(notice)
    };
    Ok(SearchOutput {
        text,
        items,
        truncated,
    })
}

async fn find(
    root: &Path,
    ripgrep: &Ripgrep,
    search_path: &Path,
    pattern: &str,
    limit: usize,
    cancellation: &CancellationToken,
) -> Result<SearchOutput, ToolError> {
    let glob = compile_glob(pattern)?;
    let mut command = ripgrep.command(root);
    command
        .args(["--files", "--null"])
        .arg("--")
        .arg(search_path);
    let mut process = SearchProcess::start(command, "ripgrep")?;
    let mut reader = BufReader::new(process.take_stdout("ripgrep")?);
    let mut output = HeadOutput::new();
    let mut buffer = Vec::new();
    let mut truncated = false;
    loop {
        buffer.clear();
        let read = tokio::select! {
            biased;
            () = cancellation.cancelled() => {
                process.stop().await?;
                return Err(ToolError::cancelled("find execution was cancelled", false));
            }
            read = reader.read_until(0, &mut buffer) => read,
        };
        let read = match read {
            Ok(read) => read,
            Err(error) => {
                return stop_with_error(&mut process, tool_error("read ripgrep output", &error))
                    .await;
            }
        };
        if read == 0 {
            break;
        }
        if buffer.last() == Some(&0) {
            buffer.pop();
        }
        let Ok(raw) = std::str::from_utf8(&buffer) else {
            return stop_with_error(
                &mut process,
                ToolError::internal("ripgrep returned a non-UTF-8 path"),
            )
            .await;
        };
        let relative = match workspace_relative(root, Path::new(raw)) {
            Ok(relative) => relative,
            Err(error) => return stop_with_error(&mut process, error).await,
        };
        if !glob.is_match(&relative) {
            continue;
        }
        if output.line_count() == limit {
            truncated = true;
            break;
        }
        let rendered = format!("{}\n", relative.to_string_lossy());
        if !output.push_line(&rendered) {
            truncated = true;
            break;
        }
    }

    let status = if truncated {
        process.stop().await?;
        None
    } else {
        Some(process.finish(cancellation, "find").await?)
    };
    if let Some(status) = status {
        process.validate_status(status, false)?;
    }
    let items = output.line_count();
    let notice =
        truncated.then_some("[Find output capped. Narrow the pattern or search path to continue.]");
    let text = if items == 0 && !truncated {
        "No files found.".to_owned()
    } else {
        output.finish(notice)
    };
    Ok(SearchOutput {
        text,
        items,
        truncated,
    })
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum RipgrepMessage {
    #[serde(rename = "match")]
    Match { data: RipgrepMatch },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct RipgrepMatch {
    path: RipgrepText,
    lines: RipgrepText,
    line_number: Option<u64>,
}

#[derive(Deserialize)]
struct RipgrepText {
    text: Option<String>,
}

fn render_match(
    root: &Path,
    data: RipgrepMatch,
    glob: Option<&GlobMatcher>,
) -> Result<Option<String>, ToolError> {
    let path = data
        .path
        .text
        .ok_or_else(|| ToolError::internal("ripgrep returned a non-UTF-8 path"))?;
    let path = workspace_relative(root, Path::new(&path))?;
    if glob.is_some_and(|glob| !glob.is_match(&path)) {
        return Ok(None);
    }
    let line = data
        .lines
        .text
        .ok_or_else(|| ToolError::internal("ripgrep returned non-UTF-8 match text"))?;
    let line = line.trim_end_matches(['\r', '\n']);
    let line_number = data
        .line_number
        .ok_or_else(|| ToolError::internal("ripgrep omitted a match line number"))?;
    Ok(Some(format!(
        "{}:{line_number}:{line}\n",
        path.to_string_lossy()
    )))
}

fn workspace_relative(root: &Path, path: &Path) -> Result<PathBuf, ToolError> {
    let relative = if path.is_absolute() {
        path.strip_prefix(root).map_err(|_| {
            ToolError::permission_denied("ripgrep returned a path outside the workspace")
        })?
    } else {
        path
    };
    Ok(relative.to_path_buf())
}

fn compile_glob(pattern: &str) -> Result<GlobMatcher, ToolError> {
    if pattern.starts_with('/') {
        return Err(ToolError::invalid_input(
            "glob pattern must be workspace-relative",
        ));
    }
    let recursive_pattern;
    let pattern = if pattern.contains('/') {
        pattern
    } else {
        recursive_pattern = format!("**/{pattern}");
        &recursive_pattern
    };
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|error| ToolError::invalid_input(format!("invalid glob pattern: {error}")))
}

async fn stop_with_error<T>(process: &mut SearchProcess, error: ToolError) -> Result<T, ToolError> {
    process.stop().await?;
    Err(error)
}

fn tool_error(action: &str, error: &std::io::Error) -> ToolError {
    io_error(action, error, false)
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;

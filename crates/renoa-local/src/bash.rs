use std::{path::PathBuf, sync::Arc};

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolExecutionMode, ToolOutput, ToolSpec,
    ToolUpdates,
};
use serde::Deserialize;
use serde_json::json;
use tokio::{process::Command, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::{
    output::{MAX_TOOL_OUTPUT_BYTES, MAX_TOOL_OUTPUT_LINES, tail},
    process::{
        CapturedTail, child_pid, configure, drain_tail, join_tail, stop_process_group,
        wait_for_process_group,
    },
    tool_input::{decode, non_empty},
};

const TRUNCATION_NOTICE: &str =
    "[Earlier command output was truncated; showing the final output.]\n";

pub(crate) struct Bash {
    root: Arc<PathBuf>,
    spec: ToolSpec,
}

impl Bash {
    pub(crate) fn new(root: Arc<PathBuf>) -> Self {
        Self {
            root,
            spec: ToolSpec {
                name: "bash".to_owned(),
                description: concat!(
                    "Run one shell command in the workspace and wait for it to finish. ",
                    "Output is capped at 2,000 lines or 50 KiB, preserving the final output."
                )
                .to_owned(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "minLength": 1 }
                    },
                    "required": ["command"],
                    "additionalProperties": false
                }),
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BashInput {
    command: String,
}

impl Tool for Bash {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Sequential
    }

    fn execute(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
        _updates: ToolUpdates,
    ) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let input: BashInput = decode(call.arguments)?;
            non_empty("command", &input.command)?;
            let mut process = Command::new("/bin/bash");
            process
                .args(["-lc", input.command.as_str()])
                .current_dir(self.root.as_ref());
            configure(&mut process);
            let mut child = process
                .spawn()
                .map_err(|error| tool_error("start shell", error))?;
            let pid = child_pid(&child)?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| ToolError::new("shell stdout was not piped"))?;
            let stderr = child
                .stderr
                .take()
                .ok_or_else(|| ToolError::new("shell stderr was not piped"))?;
            let stdout = drain_tail(stdout);
            let stderr = drain_tail(stderr);
            let exit = tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    stop_process_group(&mut child, pid).await?;
                    ProcessExit::Cancelled
                }
                status = child.wait() => ProcessExit::Finished(
                    status.map_err(|error| tool_error("wait for shell", error))?
                ),
            };
            let ProcessExit::Finished(status) = exit else {
                collect_completion(pid, stdout, stderr).await?;
                return Err(cancelled_error());
            };
            let completion = collect_completion(pid, stdout, stderr);
            tokio::pin!(completion);
            let (stdout, stderr) = tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    stop_process_group(&mut child, pid).await?;
                    completion.await?;
                    return Err(cancelled_error());
                }
                output = &mut completion => output?,
            };
            let rendered = render_process_output(&stdout, &stderr, status.code());
            if !status.success() {
                return Err(ToolError::new(rendered));
            }
            Ok(ToolOutput {
                content: vec![ContentBlock::text(rendered)],
                details: Some(json!({
                    "exit_code": status.code(),
                    "stdout_bytes": stdout.total_bytes,
                    "stderr_bytes": stderr.total_bytes,
                    "stdout_truncated": stdout.truncated(),
                    "stderr_truncated": stderr.truncated()
                })),
            })
        })
    }
}

enum ProcessExit {
    Cancelled,
    Finished(std::process::ExitStatus),
}

async fn collect_completion(
    pid: u32,
    stdout: JoinHandle<std::io::Result<CapturedTail>>,
    stderr: JoinHandle<std::io::Result<CapturedTail>>,
) -> Result<(CapturedTail, CapturedTail), ToolError> {
    let (group, stdout, stderr) = tokio::join!(
        wait_for_process_group(pid),
        join_tail(stdout),
        join_tail(stderr)
    );
    group?;
    Ok((stdout?, stderr?))
}

fn cancelled_error() -> ToolError {
    ToolError::new("bash execution was cancelled after its process group stopped")
}

fn render_process_output(
    stdout: &CapturedTail,
    stderr: &CapturedTail,
    code: Option<i32>,
) -> String {
    let code = code.map_or_else(|| "unknown".to_owned(), |value| value.to_string());
    let header = format!("Process exited with code {code}.\n");
    let mut body = String::new();
    append_output(&mut body, "stdout", stdout);
    append_output(&mut body, "stderr", stderr);

    let payload_limit = MAX_TOOL_OUTPUT_BYTES
        .saturating_sub(header.len())
        .saturating_sub(TRUNCATION_NOTICE.len());
    let (visible, additionally_truncated) = tail(&body, payload_limit, MAX_TOOL_OUTPUT_LINES);
    let truncated = additionally_truncated || stdout.truncated() || stderr.truncated();
    let mut rendered = String::with_capacity(MAX_TOOL_OUTPUT_BYTES);
    rendered.push_str(&header);
    if truncated {
        rendered.push_str(TRUNCATION_NOTICE);
    }
    rendered.push_str(visible);
    rendered
}

fn append_output(rendered: &mut String, label: &str, output: &CapturedTail) {
    if output.bytes.is_empty() {
        return;
    }
    rendered.push_str(label);
    rendered.push_str(":\n");
    rendered.push_str(&String::from_utf8_lossy(&output.bytes));
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
}

fn tool_error(action: &str, error: impl std::fmt::Display) -> ToolError {
    ToolError::new(format!("cannot {action}: {error}"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use renoa_agent::{ContentBlock, ToolCall, invoke_tool};
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    use super::Bash;
    use crate::output::MAX_TOOL_OUTPUT_BYTES;

    #[tokio::test]
    async fn large_shell_output_is_bounded_and_keeps_the_tail() {
        let directory = tempdir().expect("temporary directory");
        let bash = Bash::new(std::sync::Arc::new(directory.path().to_path_buf()));
        let call = ToolCall {
            id: "bash-output".to_owned(),
            name: "bash".to_owned(),
            arguments: serde_json::json!({
                "command": "printf 'early-marker\\n'; head -c 60000 /dev/zero | tr '\\0' x; printf '\\nfinal-marker\\n'"
            }),
            thought_signature: None,
            namespace: None,
        };

        let result = invoke_tool(Some(&bash), call, CancellationToken::new(), None)
            .await
            .expect("shell result is definite");
        let [ContentBlock::Text { text }] = result.content.as_slice() else {
            panic!("bash did not return one text block")
        };

        assert!(text.len() <= MAX_TOOL_OUTPUT_BYTES);
        assert!(text.contains("final-marker"));
        assert!(text.contains("Earlier command output was truncated"));
    }

    #[tokio::test]
    async fn shell_can_access_hidden_workspace_files() {
        let directory = tempdir().expect("temporary directory");
        fs::write(directory.path().join(".hidden"), "hidden-marker\n")
            .expect("write hidden fixture");
        let bash = Bash::new(std::sync::Arc::new(directory.path().to_path_buf()));
        let call = ToolCall {
            id: "bash-hidden".to_owned(),
            name: "bash".to_owned(),
            arguments: serde_json::json!({ "command": "cat -- .hidden" }),
            thought_signature: None,
            namespace: None,
        };

        let result = invoke_tool(Some(&bash), call, CancellationToken::new(), None)
            .await
            .expect("shell result is definite");
        assert!(!result.is_error);
        let [ContentBlock::Text { text }] = result.content.as_slice() else {
            panic!("bash did not return one text block")
        };
        assert!(text.contains("hidden-marker"));
    }
}

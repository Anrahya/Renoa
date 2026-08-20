use std::{path::PathBuf, sync::Arc, time::Duration};

use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolExecutionMode, ToolOutput, ToolSpec,
    ToolUpdates,
};
use serde::{Deserialize, Deserializer};
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
const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
const MAX_TIMEOUT_SECONDS: u64 = 1_800;

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
                description: format!(
                    "Run one shell command in the workspace and wait for it to finish. \
The default timeout is {DEFAULT_TIMEOUT_SECONDS} seconds; timeout_seconds may set it from 1 \
to {MAX_TIMEOUT_SECONDS} seconds. Output is capped at 2,000 lines or 50 KiB, preserving the \
final output."
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "minLength": 1 },
                        "timeout_seconds": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": MAX_TIMEOUT_SECONDS,
                            "default": DEFAULT_TIMEOUT_SECONDS,
                            "description": "Maximum execution time in seconds."
                        }
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
    #[serde(default, deserialize_with = "deserialize_timeout")]
    timeout_seconds: Option<u64>,
}

fn deserialize_timeout<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    u64::deserialize(deserializer).map(Some)
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
            let timeout_seconds = resolve_timeout(input.timeout_seconds)?;
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
            let deadline = tokio::time::sleep(Duration::from_secs(timeout_seconds));
            tokio::pin!(deadline);
            let exit = tokio::select! {
                biased;
                () = cancellation.cancelled() => {
                    stop_process_group(&mut child, pid).await?;
                    ProcessExit::Cancelled
                }
                status = child.wait() => ProcessExit::Finished(
                    status.map_err(|error| tool_error("wait for shell", error))?
                ),
                () = &mut deadline => {
                    stop_process_group(&mut child, pid).await?;
                    ProcessExit::TimedOut
                }
            };
            let status = match exit {
                ProcessExit::Cancelled => {
                    collect_completion(pid, stdout, stderr).await?;
                    return Err(cancelled_error());
                }
                ProcessExit::TimedOut => {
                    let (stdout, stderr) = collect_completion(pid, stdout, stderr).await?;
                    return Err(timeout_error(timeout_seconds, &stdout, &stderr));
                }
                ProcessExit::Finished(status) => status,
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
                () = &mut deadline => {
                    stop_process_group(&mut child, pid).await?;
                    let (stdout, stderr) = completion.await?;
                    return Err(timeout_error(timeout_seconds, &stdout, &stderr));
                }
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
    TimedOut,
    Finished(std::process::ExitStatus),
}

fn resolve_timeout(requested: Option<u64>) -> Result<u64, ToolError> {
    let seconds = requested.unwrap_or(DEFAULT_TIMEOUT_SECONDS);
    if !(1..=MAX_TIMEOUT_SECONDS).contains(&seconds) {
        return Err(ToolError::new(format!(
            "timeout_seconds must be between 1 and {MAX_TIMEOUT_SECONDS}"
        )));
    }
    Ok(seconds)
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

fn timeout_error(timeout_seconds: u64, stdout: &CapturedTail, stderr: &CapturedTail) -> ToolError {
    let unit = if timeout_seconds == 1 {
        "second"
    } else {
        "seconds"
    };
    let header = format!(
        "Command timed out after {timeout_seconds} {unit}. Its process group was stopped. \
The command may have made partial changes before it stopped.\n"
    );
    ToolError::new(render_captured_output(&header, stdout, stderr))
}

fn render_process_output(
    stdout: &CapturedTail,
    stderr: &CapturedTail,
    code: Option<i32>,
) -> String {
    let code = code.map_or_else(|| "unknown".to_owned(), |value| value.to_string());
    let header = format!("Process exited with code {code}.\n");
    render_captured_output(&header, stdout, stderr)
}

fn render_captured_output(header: &str, stdout: &CapturedTail, stderr: &CapturedTail) -> String {
    let mut body = String::new();
    append_output(&mut body, "stdout", stdout);
    append_output(&mut body, "stderr", stderr);

    let payload_limit = MAX_TOOL_OUTPUT_BYTES
        .saturating_sub(header.len())
        .saturating_sub(TRUNCATION_NOTICE.len());
    let (visible, additionally_truncated) = tail(&body, payload_limit, MAX_TOOL_OUTPUT_LINES);
    let truncated = additionally_truncated || stdout.truncated() || stderr.truncated();
    let mut rendered = String::with_capacity(MAX_TOOL_OUTPUT_BYTES);
    rendered.push_str(header);
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

    use renoa_agent::{ContentBlock, Tool, ToolCall, invoke_tool};
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    use super::Bash;
    use crate::output::MAX_TOOL_OUTPUT_BYTES;

    #[test]
    fn timeout_contract_is_bounded_and_unambiguous() {
        let directory = tempdir().expect("temporary directory");
        let bash = Bash::new(std::sync::Arc::new(directory.path().to_path_buf()));

        assert_eq!(
            bash.spec().input_schema["properties"]["timeout_seconds"],
            serde_json::json!({
                "type": "integer",
                "minimum": 1,
                "maximum": 1800,
                "default": 120,
                "description": "Maximum execution time in seconds."
            })
        );
    }

    #[tokio::test]
    async fn timeout_input_is_validated_before_the_command_starts() {
        let directory = tempdir().expect("temporary directory");
        let bash = Bash::new(std::sync::Arc::new(directory.path().to_path_buf()));

        for timeout in [
            serde_json::json!(0),
            serde_json::json!(1801),
            serde_json::json!(1.5),
            serde_json::json!("1"),
            serde_json::Value::Null,
        ] {
            let call = ToolCall {
                id: "bash-invalid-timeout".to_owned(),
                name: "bash".to_owned(),
                arguments: serde_json::json!({
                    "command": "printf ran > should-not-exist",
                    "timeout_seconds": timeout
                }),
                thought_signature: None,
                namespace: None,
            };
            let result = invoke_tool(Some(&bash), call, CancellationToken::new(), None)
                .await
                .expect("invalid input has a definite result");

            assert!(result.is_error);
        }
        assert!(!directory.path().join("should-not-exist").exists());

        let accepted = invoke_tool(
            Some(&bash),
            ToolCall {
                id: "bash-maximum-timeout".to_owned(),
                name: "bash".to_owned(),
                arguments: serde_json::json!({
                    "command": "printf accepted",
                    "timeout_seconds": 1800
                }),
                thought_signature: None,
                namespace: None,
            },
            CancellationToken::new(),
            None,
        )
        .await
        .expect("maximum timeout has a definite result");
        assert!(!accepted.is_error);
    }

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

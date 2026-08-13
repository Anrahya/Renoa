use std::{io, path::PathBuf, process::Stdio, sync::Arc, time::Duration};

use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use renoa_agent::{
    BoxFuture, ContentBlock, Tool, ToolCall, ToolError, ToolExecutionMode, ToolOutput, ToolSpec,
    ToolUpdates,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

const OUTPUT_LIMIT: usize = 1_000_000;

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
                description: "Run one shell command in the workspace and wait for it to finish."
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
            let command = string_argument(&call.arguments, "command")?;
            let mut process = Command::new("/bin/sh");
            process
                .args(["-lc", command])
                .current_dir(self.root.as_ref())
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            std::os::unix::process::CommandExt::process_group(process.as_std_mut(), 0);
            let mut child = process
                .spawn()
                .map_err(|error| tool_error("start shell", error))?;
            let pid = child
                .id()
                .ok_or_else(|| ToolError::new("spawned shell has no process id"))?;
            let stdout = drain(child.stdout.take().expect("piped stdout"));
            let stderr = drain(child.stderr.take().expect("piped stderr"));
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
                    "stdout_truncated": stdout.truncated,
                    "stderr_truncated": stderr.truncated
                })),
            })
        })
    }
}

enum ProcessExit {
    Cancelled,
    Finished(std::process::ExitStatus),
}

async fn stop_process_group(child: &mut Child, pid: u32) -> Result<(), ToolError> {
    signal_process_group(pid, Signal::SIGTERM)?;
    if let Ok(result) = tokio::time::timeout(Duration::from_millis(500), async {
        child
            .wait()
            .await
            .map_err(|error| tool_error("reap cancelled shell", error))?;
        wait_for_process_group(pid).await
    })
    .await
    {
        return result;
    }
    signal_process_group(pid, Signal::SIGKILL)?;
    child
        .wait()
        .await
        .map_err(|error| tool_error("reap killed shell", error))?;
    wait_for_process_group(pid).await
}

async fn collect_completion(
    pid: u32,
    stdout: JoinHandle<io::Result<CapturedOutput>>,
    stderr: JoinHandle<io::Result<CapturedOutput>>,
) -> Result<(CapturedOutput, CapturedOutput), ToolError> {
    let (group, stdout, stderr) = tokio::join!(
        wait_for_process_group(pid),
        join_output(stdout),
        join_output(stderr)
    );
    group?;
    Ok((stdout?, stderr?))
}

async fn wait_for_process_group(pid: u32) -> Result<(), ToolError> {
    let pid = process_group_id(pid)?;
    loop {
        match killpg(pid, None) {
            Err(nix::errno::Errno::ESRCH) => return Ok(()),
            Ok(()) => tokio::time::sleep(Duration::from_millis(10)).await,
            Err(error) => {
                return Err(ToolError::new(format!(
                    "cannot inspect shell process group: {error}"
                )));
            }
        }
    }
}

fn signal_process_group(pid: u32, signal: Signal) -> Result<(), ToolError> {
    match killpg(process_group_id(pid)?, signal) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(error) => Err(ToolError::new(format!(
            "cannot signal shell process group: {error}"
        ))),
    }
}

fn process_group_id(pid: u32) -> Result<Pid, ToolError> {
    i32::try_from(pid)
        .map(Pid::from_raw)
        .map_err(|_| ToolError::new("shell process id is out of range"))
}

fn cancelled_error() -> ToolError {
    ToolError::new("bash execution was cancelled after its process group stopped")
}

struct CapturedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

fn drain(
    mut reader: impl AsyncRead + Unpin + Send + 'static,
) -> JoinHandle<io::Result<CapturedOutput>> {
    tokio::spawn(async move {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 8_192];
        let mut truncated = false;
        loop {
            let read = reader.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            let remaining = OUTPUT_LIMIT.saturating_sub(bytes.len());
            let retained = read.min(remaining);
            bytes.extend_from_slice(&buffer[..retained]);
            truncated |= retained < read;
        }
        Ok(CapturedOutput { bytes, truncated })
    })
}

async fn join_output(
    output: JoinHandle<io::Result<CapturedOutput>>,
) -> Result<CapturedOutput, ToolError> {
    output
        .await
        .map_err(|error| ToolError::new(format!("output reader failed: {error}")))?
        .map_err(|error| tool_error("read process output", error))
}

fn render_process_output(
    stdout: &CapturedOutput,
    stderr: &CapturedOutput,
    code: Option<i32>,
) -> String {
    let code = code.map_or_else(|| "unknown".to_owned(), |value| value.to_string());
    let mut rendered = format!("Process exited with code {code}.");
    append_output(&mut rendered, "stdout", stdout);
    append_output(&mut rendered, "stderr", stderr);
    rendered
}

fn append_output(rendered: &mut String, label: &str, output: &CapturedOutput) {
    if output.bytes.is_empty() {
        return;
    }
    rendered.push('\n');
    rendered.push_str(label);
    rendered.push_str(":\n");
    rendered.push_str(&String::from_utf8_lossy(&output.bytes));
    if output.truncated {
        rendered.push_str("\n[");
        rendered.push_str(label);
        rendered.push_str(" truncated]");
    }
}

fn string_argument<'a>(arguments: &'a Value, name: &str) -> Result<&'a str, ToolError> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::new(format!("{name} must be a string")))
}

fn tool_error(action: &str, error: impl std::fmt::Display) -> ToolError {
    ToolError::new(format!("cannot {action}: {error}"))
}

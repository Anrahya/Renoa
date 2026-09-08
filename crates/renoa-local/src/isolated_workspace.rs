//! Per-call inspection isolation. No worker remains alive during inference.
use renoa_agent::{
    BoxFuture, Tool, ToolCall, ToolError, ToolOutput, ToolResult, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::EffectRecovery;
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    process::Command,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[cfg(test)]
#[path = "isolated_workspace_tests.rs"]
mod tests;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectionSandboxConfig {
    pub bubblewrap: PathBuf,
    pub worker: PathBuf,
}

pub(crate) struct InspectionSandbox {
    config: InspectionSandboxConfig,
    identity: String,
    specs: Vec<ToolSpec>,
    checkout: PathBuf,
}

impl InspectionSandbox {
    pub(crate) async fn start(
        config: &InspectionSandboxConfig,
        id: Uuid,
        checkout: &Path,
        cancel: &CancellationToken,
    ) -> io::Result<Self> {
        if !config.bubblewrap.is_absolute()
            || !config.worker.is_absolute()
            || !checkout.is_absolute()
        {
            return Err(io::Error::other("inspection paths must be absolute"));
        }
        let mut version = Command::new(&config.bubblewrap);
        version.arg("--version");
        let bytes = checked_output(version, &[], cancel).await?;
        let version = String::from_utf8(bytes).map_err(io::Error::other)?;
        let parts: Vec<u32> = version
            .trim()
            .strip_prefix("bubblewrap ")
            .ok_or_else(|| io::Error::other("unrecognized Bubblewrap version"))?
            .split('.')
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(io::Error::other)?;
        if parts.as_slice() < [0, 12, 0].as_slice() {
            return Err(io::Error::other("Bubblewrap 0.12.0 or newer is required"));
        }
        let worker = tokio::fs::read(&config.worker).await?;
        let mut sandbox = Self {
            config: config.clone(),
            identity: format!(
                "{id}/{}/{}",
                version.trim(),
                crate::workspace::hex_sha256(&worker)
            ),
            specs: Vec::new(),
            checkout: checkout.to_owned(),
        };
        sandbox.specs =
            serde_json::from_slice(&checked_output(sandbox.command(), &[], cancel).await?)
                .map_err(io::Error::other)?;
        Ok(sandbox)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.config.bubblewrap);
        command
            .env_clear()
            .args([
                "--unshare-all",
                "--unshare-user",
                "--disable-userns",
                "--die-with-parent",
                "--new-session",
                "--cap-drop",
                "ALL",
                "--clearenv",
                "--setenv",
                "PATH",
                "/usr/bin",
                "--ro-bind",
                "/usr/bin/rg",
                "/usr/bin/rg",
                "--ro-bind",
                "/usr/lib",
                "/usr/lib",
                "--ro-bind-try",
                "/usr/lib64",
                "/usr/lib64",
                "--symlink",
                "usr/lib",
                "/lib",
                "--symlink",
                "usr/lib64",
                "/lib64",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--tmpfs",
                "/tmp",
                "--ro-bind",
            ])
            .arg(&self.checkout)
            .arg("/workspace")
            .arg("--ro-bind")
            .arg(&self.config.worker)
            .arg("/renoa-workspace-tool")
            .args([
                "--chdir",
                "/workspace",
                "--",
                "/renoa-workspace-tool",
                "/workspace",
            ]);
        command
    }

    pub(crate) fn checkout(&self) -> &Path {
        &self.checkout
    }

    pub(crate) fn bindings(self: &Arc<Self>) -> Vec<AgentToolBinding> {
        self.specs
            .iter()
            .map(|spec| {
                AgentToolBinding::new(
                    format!("renoa.inspection/v2/{}/{}", self.identity, spec.name),
                    Arc::new(InspectionTool {
                        sandbox: Arc::clone(self),
                        spec: spec.clone(),
                    }),
                    EffectRecovery::SafeToReplay,
                )
            })
            .collect()
    }
}

struct InspectionTool {
    sandbox: Arc<InspectionSandbox>,
    spec: ToolSpec,
}
impl Tool for InspectionTool {
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
            let input =
                serde_json::to_vec(&call).map_err(|e| ToolError::invalid_input(e.to_string()))?;
            let bytes = checked_output(self.sandbox.command(), &input, &cancellation)
                .await
                .map_err(|error| {
                    if error.kind() == io::ErrorKind::Interrupted {
                        ToolError::cancelled("inspection cancelled", false)
                    } else {
                        ToolError::unavailable(error.to_string())
                    }
                })?;
            let result: ToolResult =
                serde_json::from_slice(&bytes).map_err(|e| ToolError::internal(e.to_string()))?;
            if result.call_id != call.id || result.name != call.name {
                return Err(ToolError::internal(
                    "inspection returned a different tool-call identity",
                ));
            }
            Ok(ToolOutput {
                content: result.content,
                details: result.details,
                is_error: result.is_error,
            })
        })
    }
}

pub(crate) async fn checked_output(
    mut command: Command,
    input: &[u8],
    cancel: &CancellationToken,
) -> io::Result<Vec<u8>> {
    if cancel.is_cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "operation cancelled",
        ));
    }
    crate::process::configure_process_group(&mut command);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let pid = crate::process::child_pid_raw(&child).map_err(io::Error::other)?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("missing process input"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing process output"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("missing process error output"))?;
    let work = async {
        let writer = async {
            stdin.write_all(input).await?;
            stdin.shutdown().await?;
            drop(stdin);
            Ok::<_, io::Error>(())
        };
        let reader = async {
            let mut bytes = Vec::new();
            stdout.take(1_048_577).read_to_end(&mut bytes).await?;
            if bytes.len() > 1_048_576 {
                return Err(io::Error::other(
                    "workspace process output exceeds transport capacity",
                ));
            }
            Ok::<_, io::Error>(bytes)
        };
        let errors = async {
            let mut bytes = Vec::new();
            stderr.take(65_537).read_to_end(&mut bytes).await?;
            if bytes.len() > 65_536 {
                return Err(io::Error::other(
                    "workspace process error output exceeds transport capacity",
                ));
            }
            Ok::<_, io::Error>(bytes)
        };
        let ((), bytes, _, status) = tokio::try_join!(writer, reader, errors, child.wait())?;
        if !status.success() {
            // Git subprocesses can mention authentication material. Keep raw
            // diagnostics outside this generic boundary's user-visible errors.
            return Err(io::Error::other(format!(
                "workspace process exited with {status}"
            )));
        }
        Ok(bytes)
    };
    let result = tokio::select! {
        result=work=>result,
        ()=cancel.cancelled()=>Err(io::Error::new(io::ErrorKind::Interrupted, "operation cancelled")),
    };
    crate::process::stop_process_group_raw(&mut child, pid)
        .await
        .map_err(io::Error::other)?;
    result
}

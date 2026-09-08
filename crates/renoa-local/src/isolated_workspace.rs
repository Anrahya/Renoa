//! Disposable inspection environment. Agent state and credentials remain with
//! the Host; the container sees only an immutable checkout and existing tools.
use std::{
    io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use renoa_agent::{
    BoxFuture, Tool, ToolCall, ToolError, ToolOutput, ToolResult, ToolSpec, ToolUpdates,
};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::EffectRecovery;
use serde::{Deserialize, Serialize};
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
pub struct InspectionContainerConfig {
    pub engine: PathBuf,
    /// A locally available image; the resolved content identity binds tool replay.
    pub image: String,
}

pub(crate) struct InspectionContainer {
    engine: PathBuf,
    name: String,
    image: String,
    specs: Vec<ToolSpec>,
    checkout: PathBuf,
}

impl InspectionContainer {
    pub(crate) async fn remove_for(config: &InspectionContainerConfig, id: Uuid) -> io::Result<()> {
        Self {
            engine: config.engine.clone(),
            name: format!("renoa-inspection-{id}"),
            image: String::new(),
            specs: Vec::new(),
            checkout: PathBuf::new(),
        }
        .remove()
        .await
    }
    pub(crate) async fn start(
        config: &InspectionContainerConfig,
        id: Uuid,
        checkout: &Path,
        cancel: &CancellationToken,
    ) -> io::Result<Self> {
        if !config.engine.is_absolute() || config.image.starts_with('-') || config.image.is_empty()
        {
            return Err(io::Error::other(
                "provide an absolute container engine and a local image",
            ));
        }
        let mut image = Command::new(&config.engine);
        image.args(["image", "inspect", "--format", "{{.Id}}", &config.image]);
        let image = String::from_utf8(checked_output(image, &[], cancel).await?)
            .map_err(io::Error::other)?
            .trim()
            .to_owned();
        let name = format!("renoa-inspection-{id}");
        let mut container = Self {
            engine: config.engine.clone(),
            name,
            image,
            specs: Vec::new(),
            checkout: checkout.to_owned(),
        };
        // A previous owner may have died after create. Recreate this read-only
        // environment; durable model/tool effects stay in the Host's kernel.
        container.remove().await?;
        let root = checkout
            .to_str()
            .ok_or_else(|| io::Error::other("checkout path is not UTF-8"))?;
        if root.contains([',', ':', '\n']) || !checkout.is_absolute() {
            return Err(io::Error::other(
                "checkout requires an absolute mount-safe path",
            ));
        }
        let mut command = Command::new(&container.engine);
        command.args([
            "run",
            "--detach",
            "--name",
            &container.name,
            "--label",
            &format!("renoa.inspection={id}"),
            "--network",
            "none",
            "--read-only",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges",
            "--user",
            "65534:65534",
            "--volume",
            &format!("{root}:/workspace:ro,Z"),
            &container.image,
        ]);
        let creation = checked_output(command, &[], cancel).await;
        if let Err(error) = creation {
            container.remove().await?;
            return Err(error);
        }
        let specs = checked_output(container.command(), &[], cancel).await;
        match specs.and_then(|bytes| serde_json::from_slice(&bytes).map_err(io::Error::other)) {
            Ok(specs) => {
                container.specs = specs;
                Ok(container)
            }
            Err(error) => {
                container.remove().await?;
                Err(error)
            }
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.engine);
        command.args([
            "exec",
            "--interactive",
            &self.name,
            "/usr/local/bin/renoa-workspace-tool",
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
                    format!(
                        "renoa.inspection/v1/{}/{}/{}",
                        self.name, self.image, spec.name
                    ),
                    Arc::new(ContainerTool {
                        container: Arc::clone(self),
                        spec: spec.clone(),
                    }),
                    EffectRecovery::SafeToReplay,
                )
            })
            .collect()
    }

    pub(crate) async fn remove(&self) -> io::Result<()> {
        let mut inspect = Command::new(&self.engine);
        inspect.args([
            "container",
            "ls",
            "--all",
            "--filter",
            &format!("name={}", self.name),
            "--format",
            "{{.Names}} {{.Label \"renoa.inspection\"}}",
        ]);
        let output = checked_output(inspect, &[], &CancellationToken::new()).await?;
        let expected = format!(
            "{} {}",
            self.name,
            self.name.trim_start_matches("renoa-inspection-")
        );
        let output = String::from_utf8(output).map_err(io::Error::other)?;
        if output.trim().is_empty() {
            return Ok(());
        }
        if output.trim() != expected {
            return Err(io::Error::other(
                "container name belongs to a different owner",
            ));
        }
        let mut remove = Command::new(&self.engine);
        remove.args(["rm", "--force", &self.name]);
        checked_output(remove, &[], &CancellationToken::new()).await?;
        Ok(())
    }
}

struct ContainerTool {
    container: Arc<InspectionContainer>,
    spec: ToolSpec,
}
impl Tool for ContainerTool {
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
            let input = serde_json::to_vec(&call)
                .map_err(|error| ToolError::invalid_input(error.to_string()))?;
            let bytes = checked_output(self.container.command(), &input, &cancellation)
                .await
                .map_err(|error| {
                    if error.kind() == io::ErrorKind::Interrupted {
                        ToolError::cancelled("inspection cancelled", false)
                    } else {
                        ToolError::unavailable(error.to_string())
                    }
                })?;
            let result: ToolResult = serde_json::from_slice(&bytes)
                .map_err(|error| ToolError::internal(error.to_string()))?;
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

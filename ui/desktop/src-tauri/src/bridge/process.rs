use std::{
    io::{BufRead as _, BufReader, Write as _},
    path::Path,
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use serde::Serialize;
use tauri::Emitter as _;

use super::BridgeRegistry;

pub(super) type SharedChild = Arc<Mutex<Option<Child>>>;

pub(super) struct SpawnedBridgeProcess {
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr: ChildStderr,
    child: SharedChild,
}

impl SpawnedBridgeProcess {
    pub(super) fn child(&self) -> SharedChild {
        Arc::clone(&self.child)
    }
}

struct ChildGuard(Option<Child>);

impl ChildGuard {
    const fn new(child: Child) -> Self {
        Self(Some(child))
    }

    fn process(&mut self) -> Result<&mut Child, String> {
        self.0
            .as_mut()
            .ok_or_else(|| "Renoa ACP child ownership was already transferred".to_owned())
    }

    fn into_shared(mut self) -> Result<Arc<Mutex<Option<Child>>>, String> {
        self.0
            .take()
            .map(|child| Arc::new(Mutex::new(Some(child))))
            .ok_or_else(|| "Renoa ACP child ownership was already transferred".to_owned())
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take()
            && let Err(error) = terminate_child(&mut child)
        {
            eprintln!("failed to clean up an unpublished Renoa ACP child: {error}");
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentOutputPayload {
    bridge_id: String,
    data: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentErrorPayload {
    bridge_id: String,
    message: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentStatusPayload {
    bridge_id: String,
}

pub(super) fn spawn_bridge_process(binary: &Path) -> Result<SpawnedBridgeProcess, String> {
    let mut command = Command::new(binary);
    command
        .arg("acp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_process_group(&mut command);
    let child = command
        .spawn()
        .map_err(|error| format!("failed to start {}: {error}", binary.display()))?;
    let mut child = ChildGuard::new(child);
    let stdin = child
        .process()?
        .stdin
        .take()
        .ok_or_else(|| "Renoa ACP stdin was unavailable".to_owned())?;
    let stdout = child
        .process()?
        .stdout
        .take()
        .ok_or_else(|| "Renoa ACP stdout was unavailable".to_owned())?;
    let stderr = child
        .process()?
        .stderr
        .take()
        .ok_or_else(|| "Renoa ACP stderr was unavailable".to_owned())?;
    let child = child.into_shared()?;
    Ok(SpawnedBridgeProcess {
        stdin,
        stdout,
        stderr,
        child,
    })
}

pub(super) fn spawn_bridge_workers(
    app: &tauri::AppHandle,
    bridge_id: &str,
    generation: u64,
    process: SpawnedBridgeProcess,
    receiver: mpsc::Receiver<String>,
    registry: Arc<Mutex<BridgeRegistry>>,
) -> Result<(), String> {
    let SpawnedBridgeProcess {
        stdin,
        stdout,
        stderr,
        child,
    } = process;

    let input_app = app.clone();
    let input_id = bridge_id.to_owned();
    spawn_worker("renoa-acp-input", move || {
        let mut stdin = stdin;
        for line in receiver {
            if let Err(error) = stdin
                .write_all(line.as_bytes())
                .and_then(|()| stdin.flush())
            {
                emit_error(&input_app, &input_id, format!("ACP input failed: {error}"));
                break;
            }
        }
    })?;

    let output_app = app.clone();
    let output_id = bridge_id.to_owned();
    spawn_worker("renoa-acp-output", move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(data) => emit_or_log(
                    &output_app,
                    "renoa-acp-output",
                    AgentOutputPayload {
                        bridge_id: output_id.clone(),
                        data,
                    },
                ),
                Err(error) => {
                    emit_error(
                        &output_app,
                        &output_id,
                        format!("ACP output failed: {error}"),
                    );
                    break;
                }
            }
        }
    })?;

    let diagnostic_app = app.clone();
    let diagnostic_id = bridge_id.to_owned();
    spawn_worker("renoa-acp-diagnostics", move || {
        for line in BufReader::new(stderr).lines() {
            match line {
                Ok(message) => {
                    eprintln!("[renoa-agent:{diagnostic_id}] {message}");
                    emit_or_log(
                        &diagnostic_app,
                        "renoa-acp-diagnostic",
                        AgentErrorPayload {
                            bridge_id: diagnostic_id.clone(),
                            message,
                        },
                    );
                }
                Err(error) => {
                    emit_error(
                        &diagnostic_app,
                        &diagnostic_id,
                        format!("ACP diagnostics failed: {error}"),
                    );
                    break;
                }
            }
        }
    })?;

    let monitor_app = app.clone();
    let monitor_id = bridge_id.to_owned();
    spawn_worker("renoa-acp-monitor", move || {
        monitor_child(&monitor_app, &monitor_id, generation, &child, &registry);
    })
}

fn spawn_worker(name: &str, work: impl FnOnce() + Send + 'static) -> Result<(), String> {
    let handle = std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(work)
        .map_err(|error| format!("failed to start {name} worker: {error}"))?;
    // Each worker owns a pipe, channel, or child handle that closes when its bridge stops.
    // Joining here would block the Tauri command that just started the process.
    drop(handle);
    Ok(())
}

fn monitor_child(
    app: &tauri::AppHandle,
    bridge_id: &str,
    generation: u64,
    child: &SharedChild,
    registry: &Arc<Mutex<BridgeRegistry>>,
) {
    loop {
        let status = {
            let mut guard = child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(process) = guard.as_mut() else {
                return;
            };
            let status = process.try_wait();
            if matches!(status, Ok(Some(_))) {
                if let Err(error) = terminate_child(process) {
                    emit_error(
                        app,
                        bridge_id,
                        format!("ACP process-tree cleanup failed after exit: {error}"),
                    );
                }
                *guard = None;
            }
            status
        };
        match status {
            Ok(Some(status)) => {
                remove_bridge(registry, bridge_id, generation);
                if status.success() {
                    emit_or_log(
                        app,
                        "renoa-acp-closed",
                        AgentStatusPayload {
                            bridge_id: bridge_id.to_owned(),
                        },
                    );
                } else {
                    emit_error(app, bridge_id, format!("ACP process exited with {status}"));
                }
                return;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => {
                mark_input_closed(registry, bridge_id, generation);
                let (message, cleaned) = match stop_child(child) {
                    Ok(()) => (format!("ACP process monitoring failed: {error}"), true),
                    Err(cleanup) => (
                        format!(
                            "ACP process monitoring failed: {error}; cleanup failed: {cleanup}"
                        ),
                        false,
                    ),
                };
                if cleaned {
                    remove_bridge(registry, bridge_id, generation);
                }
                emit_error(app, bridge_id, message);
                return;
            }
        }
    }
}

fn mark_input_closed(registry: &Arc<Mutex<BridgeRegistry>>, bridge_id: &str, generation: u64) {
    let mut registry = registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    registry.close_input_if_generation(bridge_id, generation);
}

fn remove_bridge(registry: &Arc<Mutex<BridgeRegistry>>, bridge_id: &str, generation: u64) {
    let mut registry = registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    drop(registry.remove_if_generation(bridge_id, generation));
}

pub(super) fn stop_child(child: &SharedChild) -> Result<(), String> {
    let mut guard = child.lock().map_err(|error| error.to_string())?;
    let Some(process) = guard.as_mut() else {
        return Ok(());
    };
    terminate_child(process)?;
    *guard = None;
    Ok(())
}

fn terminate_child(child: &mut Child) -> Result<(), String> {
    terminate_process_tree(child)
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;

    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_process_tree(child: &mut Child) -> Result<(), String> {
    use nix::{
        errno::Errno,
        sys::signal::{Signal, killpg},
        unistd::Pid,
    };

    let pid = i32::try_from(child.id())
        .map(Pid::from_raw)
        .map_err(|_| "Renoa ACP process id exceeded i32".to_owned())?;
    let already_exited = child
        .try_wait()
        .map_err(|error| format!("failed to inspect Renoa ACP child: {error}"))?
        .is_some();
    if !already_exited
        && let Err(error) = killpg(pid, Signal::SIGTERM)
        && error != Errno::ESRCH
    {
        return Err(format!(
            "failed to terminate Renoa ACP process group: {error}"
        ));
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut reaped = already_exited;
    while !reaped && std::time::Instant::now() < deadline {
        reaped = child
            .try_wait()
            .map_err(|error| format!("failed to inspect Renoa ACP child: {error}"))?
            .is_some();
        if !reaped {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    if let Err(error) = killpg(pid, Signal::SIGKILL)
        && error != Errno::ESRCH
    {
        return Err(format!("failed to kill Renoa ACP process group: {error}"));
    }
    if !reaped {
        child
            .wait()
            .map_err(|error| format!("failed to reap Renoa ACP child: {error}"))?;
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match killpg(pid, None) {
            Err(Errno::ESRCH) => return Ok(()),
            Ok(()) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(()) => {
                return Err(
                    "Renoa ACP process group survived its 2-second kill deadline".to_owned(),
                );
            }
            Err(error) => {
                return Err(format!(
                    "failed to inspect Renoa ACP process group: {error}"
                ));
            }
        }
    }
}

#[cfg(not(unix))]
fn terminate_process_tree(child: &mut Child) -> Result<(), String> {
    if child
        .try_wait()
        .map_err(|error| format!("failed to inspect Renoa ACP child: {error}"))?
        .is_some()
    {
        return Ok(());
    }
    child
        .kill()
        .map_err(|error| format!("failed to stop Renoa ACP child: {error}"))?;
    child
        .wait()
        .map_err(|error| format!("failed to reap Renoa ACP child: {error}"))?;
    Ok(())
}

fn emit_error(app: &tauri::AppHandle, bridge_id: &str, message: String) {
    emit_or_log(
        app,
        "renoa-acp-error",
        AgentErrorPayload {
            bridge_id: bridge_id.to_owned(),
            message,
        },
    );
}

fn emit_or_log<T: Clone + Serialize>(app: &tauri::AppHandle, event: &str, payload: T) {
    if let Err(error) = app.emit(event, payload) {
        eprintln!("failed to emit Tauri event {event}: {error}");
    }
}

#[cfg(all(test, unix))]
#[path = "process_tests.rs"]
mod tests;

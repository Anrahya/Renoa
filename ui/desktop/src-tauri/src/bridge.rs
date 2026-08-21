use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, SyncSender, TrySendError},
    },
};

use serde::Deserialize;
use tauri::Manager as _;

mod process;

use process::{SharedChild, spawn_bridge_process, spawn_bridge_workers, stop_child};

const INPUT_QUEUE_CAPACITY: usize = 64;

struct AgentBridge {
    generation: u64,
    input: Option<SyncSender<String>>,
    child: SharedChild,
}

#[derive(Default)]
struct BridgeRegistry {
    next_generation: u64,
    agents: HashMap<String, AgentBridge>,
}

#[derive(Default)]
pub(crate) struct BridgeState {
    registry: Arc<Mutex<BridgeRegistry>>,
}

impl BridgeRegistry {
    fn allocate_generation(&mut self) -> Result<u64, String> {
        let generation = self.next_generation;
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or_else(|| "Renoa ACP bridge generation overflowed".to_owned())?;
        Ok(generation)
    }

    fn remove_if_generation(&mut self, bridge_id: &str, generation: u64) -> Option<AgentBridge> {
        if self
            .agents
            .get(bridge_id)
            .is_some_and(|bridge| bridge.generation == generation)
        {
            self.agents.remove(bridge_id)
        } else {
            None
        }
    }

    fn close_input_if_generation(&mut self, bridge_id: &str, generation: u64) {
        if let Some(bridge) = self.agents.get_mut(bridge_id)
            && bridge.generation == generation
        {
            drop(bridge.input.take());
        }
    }
}

impl Drop for BridgeState {
    fn drop(&mut self) {
        let bridges = {
            let mut registry = self
                .registry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            registry
                .agents
                .drain()
                .map(|(_, bridge)| bridge)
                .collect::<Vec<_>>()
        };
        for bridge in bridges {
            drop(bridge.input);
            if let Err(error) = stop_child(&bridge.child) {
                eprintln!("failed to stop Renoa ACP bridge during shutdown: {error}");
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartAgentArgs {
    bridge_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentActionArgs {
    bridge_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WriteToAgentArgs {
    bridge_id: String,
    line: String,
}

fn executable_name() -> &'static str {
    if cfg!(windows) {
        "renoa-agent.exe"
    } else {
        "renoa-agent"
    }
}

fn existing_candidate(path: PathBuf) -> Option<PathBuf> {
    path.is_file().then_some(path)
}

fn resolve_agent_binary(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Some(configured) = std::env::var_os("RENOA_AGENT_BIN") {
        let path = PathBuf::from(configured);
        if !path.is_absolute() || !path.is_file() {
            return Err("RENOA_AGENT_BIN must name an existing absolute file".to_owned());
        }
        return Ok(path);
    }

    let name = executable_name();
    if let Ok(resource_dir) = app.path().resource_dir()
        && let Some(path) = existing_candidate(resource_dir.join(name))
    {
        return Ok(path);
    }
    if let Ok(current_exe) = std::env::current_exe()
        && let Some(parent) = current_exe.parent()
        && let Some(path) = existing_candidate(parent.join(name))
    {
        return Ok(path);
    }

    let development = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join("target/debug")
        .join(name);
    if let Some(path) = existing_candidate(development) {
        return Ok(path);
    }

    Err(format!(
        "{name} was not found; set RENOA_AGENT_BIN to its absolute path"
    ))
}

#[tauri::command]
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri command injection supplies state, app handles, and decoded arguments by value"
)]
pub(crate) fn start_agent(
    state: tauri::State<'_, BridgeState>,
    app: tauri::AppHandle,
    args: StartAgentArgs,
) -> Result<(), String> {
    let binary = resolve_agent_binary(&app)?;
    let mut registry = state.registry.lock().map_err(|error| error.to_string())?;
    if registry.agents.contains_key(&args.bridge_id) {
        return Err("the Renoa ACP bridge is already running".to_owned());
    }
    let generation = registry.allocate_generation()?;
    let process = spawn_bridge_process(&binary)?;
    let child = process.child();
    let (input, receiver) = mpsc::sync_channel::<String>(INPUT_QUEUE_CAPACITY);
    match registry.agents.entry(args.bridge_id.clone()) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(AgentBridge {
                generation,
                input: Some(input),
                child: Arc::clone(&child),
            });
        }
        std::collections::hash_map::Entry::Occupied(_) => {
            let source = "the Renoa ACP bridge became occupied while starting";
            return match stop_child(&child) {
                Ok(()) => Err(source.to_owned()),
                Err(cleanup) => Err(format!(
                    "{source}; failed to clean up the Renoa ACP child: {cleanup}"
                )),
            };
        }
    }

    let workers = spawn_bridge_workers(
        &app,
        &args.bridge_id,
        generation,
        process,
        receiver,
        Arc::clone(&state.registry),
    );
    if let Err(source) = workers {
        drop(registry.remove_if_generation(&args.bridge_id, generation));
        let cleanup = stop_child(&child);
        return match cleanup {
            Ok(()) => Err(source),
            Err(cleanup) => Err(format!(
                "{source}; failed to clean up the Renoa ACP child: {cleanup}"
            )),
        };
    }
    Ok(())
}

#[tauri::command]
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri command injection supplies state and decoded arguments by value"
)]
pub(crate) fn write_to_agent(
    state: tauri::State<'_, BridgeState>,
    args: WriteToAgentArgs,
) -> Result<(), String> {
    let registry = state.registry.lock().map_err(|error| error.to_string())?;
    let bridge = registry
        .agents
        .get(&args.bridge_id)
        .ok_or_else(|| "the Renoa ACP bridge is not running".to_owned())?;
    match bridge
        .input
        .as_ref()
        .ok_or_else(|| "the Renoa ACP input is closed".to_owned())?
        .try_send(args.line)
    {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err("the Renoa ACP input queue is full".to_owned()),
        Err(TrySendError::Disconnected(_)) => {
            Err("the Renoa ACP input channel is closed".to_owned())
        }
    }
}

#[tauri::command]
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri command injection supplies state and decoded arguments by value"
)]
pub(crate) fn kill_agent(
    state: tauri::State<'_, BridgeState>,
    args: AgentActionArgs,
) -> Result<(), String> {
    let (generation, child) = {
        let mut registry = state.registry.lock().map_err(|error| error.to_string())?;
        let Some(bridge) = registry.agents.get_mut(&args.bridge_id) else {
            return Ok(());
        };
        drop(bridge.input.take());
        (bridge.generation, Arc::clone(&bridge.child))
    };
    stop_child(&child)?;
    let mut registry = state.registry.lock().map_err(|error| error.to_string())?;
    drop(registry.remove_if_generation(&args.bridge_id, generation));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_monitor_cannot_remove_a_new_bridge_generation() {
        let (input, _receiver) = mpsc::sync_channel(1);
        let mut registry = BridgeRegistry::default();
        registry.agents.insert(
            "main".to_owned(),
            AgentBridge {
                generation: 2,
                input: Some(input),
                child: Arc::new(Mutex::new(None)),
            },
        );

        assert!(registry.remove_if_generation("main", 1).is_none());
        assert_eq!(registry.agents["main"].generation, 2);
        assert!(registry.remove_if_generation("main", 2).is_some());
    }
}

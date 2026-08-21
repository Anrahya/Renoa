#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bridge;

use bridge::{BridgeState, kill_agent, start_agent, write_to_agent};

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(BridgeState::default())
        .invoke_handler(tauri::generate_handler![
            start_agent,
            write_to_agent,
            kill_agent,
        ])
        .run(tauri::generate_context!())
        .expect("Renoa desktop failed to start");
}

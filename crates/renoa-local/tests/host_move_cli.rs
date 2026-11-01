//! The owner's `move-to-preset` command, run as the Host binary.

use std::{fs, path::Path, process::Command};

use serde_json::{Value, json};

/// Writes one Host configuration that needs no provider inference.
fn fixture(root: &Path) {
    let data = root.join("data");
    let bridge = root.join("model.mjs");
    let auth = root.join("auth.sqlite");
    fs::write(
        &bridge,
        "throw new Error('a move must not invoke inference');",
    )
    .expect("model");
    fs::write(&auth, "").expect("auth boundary");
    fs::write(
        root.join("host.json"),
        serde_json::to_vec(&json!({
            "home":data,"model_bridge":bridge,"providers":["xai"],"provider":"xai",
            "model":"fixture","model_auth_store":auth
        }))
        .expect("config"),
    )
    .expect("config file");
}

fn host(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_renoa-host"))
        .arg(root.join("host.json"))
        .args(args)
        .output()
        .expect("Host CLI")
}

fn json_of(output: &std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON output")
}

#[test]
fn move_to_preset_moves_once_and_then_reports_no_change() {
    let directory = tempfile::tempdir().expect("fixture");
    let root = directory.path();
    fixture(root);
    let document = root.join("provision.json");
    fs::write(
        &document,
        serde_json::to_vec(&json!({
            "operationId": uuid::Uuid::new_v4(),
            "presetId": "renoa.general.v1",
            "name": "Renoa"
        }))
        .expect("document"),
    )
    .expect("provision file");
    let document = document.to_str().expect("UTF-8 path");
    let agent = json_of(&host(root, &["provision", document]))["id"]
        .as_str()
        .expect("agent id")
        .to_owned();

    let moved = json_of(&host(root, &["move-to-preset", &agent, "renoa.app.v2"]));
    assert_eq!(moved["changed"], true);
    assert_eq!(moved["definition"]["id"], agent.as_str());
    assert_eq!(moved["definition"]["name"], "Renoa");
    assert_eq!(moved["definition"]["preset_id"], "renoa.app.v2");
    assert!(
        root.join("data/agents")
            .join(&agent)
            .join("SOUL.md")
            .is_file()
    );

    let again = json_of(&host(root, &["move-to-preset", &agent, "renoa.app.v2"]));
    assert_eq!(again["changed"], false);
    assert_eq!(again["definition"], moved["definition"]);

    let unknown = host(root, &["move-to-preset", &agent, "renoa.unknown.v1"]);
    assert!(!unknown.status.success());
    assert!(
        String::from_utf8_lossy(&unknown.stderr).contains("renoa.unknown.v1"),
        "{}",
        String::from_utf8_lossy(&unknown.stderr)
    );
    let malformed = host(root, &["move-to-preset", "not-an-agent", "renoa.app.v2"]);
    assert!(!malformed.status.success());
}

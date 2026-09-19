use std::{fs, path::Path, process::Command};

use serde_json::json;

/// Writes one Host configuration that needs no provider inference.
fn fixture(root: &Path) {
    let data = root.join("data");
    let bridge = root.join("model.mjs");
    let auth = root.join("auth.sqlite");
    fs::write(
        &bridge,
        "throw new Error('reset must not invoke inference');",
    )
    .expect("model");
    fs::write(&auth, "").expect("auth boundary");
    fs::write(
        root.join("host.json"),
        serde_json::to_vec(&json!({
            "data_directory":data,"model_bridge":bridge,"providers":["xai"],"provider":"xai",
            "model":"fixture","model_auth_store":auth
        }))
        .expect("config"),
    )
    .expect("config file");
}

fn provision(root: &Path) {
    let document = root.join("provision.json");
    fs::write(
        &document,
        serde_json::to_vec(&json!({
            "operationId": uuid::Uuid::new_v4(),
            "presetId": "renoa.coding.alpha.v1",
            "name": "Alpha"
        }))
        .expect("document"),
    )
    .expect("provision file");
    let output = Command::new(env!("CARGO_BIN_EXE_renoa-host"))
        .arg(root.join("host.json"))
        .arg("provision")
        .arg(&document)
        .output()
        .expect("Host CLI");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn reset(root: &Path, backup: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_renoa-host"))
        .arg(root.join("host.json"))
        .arg("reset")
        .arg(backup)
        .output()
        .expect("Host CLI")
}

#[test]
fn a_reset_backs_up_the_data_root_first_and_preserves_shared_state() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        reset_scenario().await;
    });
}

async fn reset_scenario() {
    let directory = tempfile::tempdir().expect("fixture");
    let root = directory.path();
    fixture(root);
    provision(root);
    let workspace = root.join("data/agent-workspaces");
    fs::create_dir_all(&workspace).expect("workspace directory");
    fs::write(workspace.join("kept.txt"), "kept\n").expect("workspace file");

    let backup = root.join("previous-release");
    let output = reset(root, &backup);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: renoa_local::HostResetReport =
        serde_json::from_slice(&output.stdout).expect("typed reset report");
    assert!(report.total_rows() >= 2, "{report:?}");
    assert!(backup.join("host.sqlite3").is_file());
    assert_eq!(
        fs::read_to_string(workspace.join("kept.txt")).expect("preserved workspace file"),
        "kept\n"
    );

    let agents = renoa_local::HostObserver::open(&root.join("data"))
        .expect("observer")
        .snapshot()
        .await
        .expect("snapshot");
    assert!(agents.agents.is_empty(), "{agents:?}");

    let replaced = reset(root, &backup);
    assert!(!replaced.status.success());
    assert!(
        String::from_utf8_lossy(&replaced.stderr).contains("not empty"),
        "{}",
        String::from_utf8_lossy(&replaced.stderr)
    );

    let nested = reset(root, &root.join("data/backup"));
    assert!(!nested.status.success());
    assert!(
        String::from_utf8_lossy(&nested.stderr).contains("must not be inside"),
        "{}",
        String::from_utf8_lossy(&nested.stderr)
    );
}

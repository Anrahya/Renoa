use std::process::Command;

use renoa_local::{
    LocalHost, LocalHostAdapters, LocalModelConfiguration, ModelProvider, alpha_profile,
};

#[tokio::test]
async fn inspection_binary_reads_an_existing_host_without_a_launch_config_or_models() {
    let root = tempfile::tempdir().expect("root");
    let host = LocalHost::new(
        root.path(),
        LocalModelConfiguration::new(
            root.path().join("absent-bridge"),
            vec![ModelProvider::Xai],
            ModelProvider::Xai,
            "absent-model",
            root.path().join("absent-credentials"),
        ),
        vec![alpha_profile()],
        LocalHostAdapters::default(),
    )
    .expect("Host");
    let output = Command::new(env!("CARGO_BIN_EXE_renoa-host"))
        .arg("inspect")
        .arg(root.path())
        .output()
        .expect("inspect binary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let snapshot: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("observation JSON");
    assert_eq!(
        snapshot["host_id"],
        host.host_id().await.expect("id").to_string()
    );
    assert_eq!(snapshot["agents"], serde_json::json!([]));
    assert_eq!(snapshot["sessions"], serde_json::json!([]));
    assert!(!root.path().join("absent-credentials").exists());
}

#[test]
fn inspection_binary_does_not_turn_a_typo_into_a_new_host() {
    let root = tempfile::tempdir().expect("root");
    let path = root.path().join("wrong-root");
    let output = Command::new(env!("CARGO_BIN_EXE_renoa-host"))
        .arg("inspect")
        .arg(&path)
        .output()
        .expect("inspect binary");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!path.exists());
}

//! The ACP surface must refuse a configured agent this Host has not provisioned.

#[allow(
    dead_code,
    reason = "the shared support module is compiled whole; this target exercises only startup"
)]
mod support;

use std::fs;

use tempfile::tempdir;
use uuid::Uuid;

use support::{AcpProcess, BRIDGE};

#[test]
fn an_unprovisioned_configured_agent_refuses_startup_before_serving() {
    let fixture = Fixture::new();
    let configured = Uuid::new_v4().to_string();

    let stderr = AcpProcess::spawn_for_agent(
        &fixture.workspace,
        &fixture.data,
        &fixture.bridge,
        &fixture.auth,
        &configured,
    )
    .expect_startup_failure();

    assert!(
        stderr.contains(&configured),
        "the refusal does not name the configured agent: {stderr}"
    );
    assert!(
        stderr.contains("renoa-host <config.json> provision <provision.json>"),
        "the refusal does not name the provisioning command: {stderr}"
    );
}

#[test]
fn a_provisioned_configured_agent_still_starts_serving() {
    let fixture = Fixture::new();
    let mut process = AcpProcess::spawn(
        &fixture.workspace,
        &fixture.data,
        &fixture.bridge,
        &fixture.auth,
    );

    let initialized = process.initialize();

    assert_eq!(initialized["result"]["protocolVersion"], 1);
    process.finish();
}

struct Fixture {
    _directory: tempfile::TempDir,
    workspace: std::path::PathBuf,
    data: std::path::PathBuf,
    bridge: std::path::PathBuf,
    auth: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempdir().expect("fixture directory");
        let workspace = directory.path().join("workspace");
        let data = directory.path().join("data");
        let bridge = directory.path().join("bridge.mjs");
        let auth = directory.path().join("auth.sqlite");
        fs::create_dir(&workspace).expect("workspace");
        fs::write(&bridge, BRIDGE).expect("fixture model bridge");
        fs::write(&auth, "").expect("fixture credentials");
        Self {
            _directory: directory,
            workspace,
            data,
            bridge,
            auth,
        }
    }
}

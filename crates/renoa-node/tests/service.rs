#[allow(
    dead_code,
    unused_imports,
    reason = "the shared node fixture exposes paths used by the larger live-bridge suite"
)]
mod support;

use std::{
    fs,
    process::{Command as StdCommand, Stdio},
    time::{Duration, SystemTime},
};

use renoa_control::{DeviceCredentials, NodeId, PeerIdentity};
use renoa_protocol::{CommandId, ExecutionEventKind, ExecutionTerminal};
use serde_json::json;
use tokio::time::timeout;

use support::{
    HostFixture, TestSystem, attach, collect_through_terminal, submit_when_node_is_online,
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

#[tokio::test]
async fn enrollment_command_writes_a_private_usable_device_credential() {
    timeout(Duration::from_secs(10), async {
        let system = TestSystem::start().await;
        let token = system
            .coordinator
            .create_enrollment(
                PeerIdentity::Node {
                    node_id: NodeId::new(),
                },
                SystemTime::now() + Duration::from_mins(1),
            )
            .await
            .expect("create enrollment");
        let enrollment = system.files.path().join("enrollment.json");
        let output = system.files.path().join("device.json");
        write_private(
            &enrollment,
            &serde_json::to_vec(&json!({"token": token})).expect("encode token"),
        );

        let completed = tokio::process::Command::new(env!("CARGO_BIN_EXE_renoa-node"))
            .args(["enroll", &system.url])
            .arg(&enrollment)
            .arg(&output)
            .output()
            .await
            .expect("run enrollment command");

        assert!(
            completed.status.success(),
            "enrollment failed: {}",
            String::from_utf8_lossy(&completed.stderr)
        );
        assert_eq!(completed.stdout, b"{\"status\":\"enrolled\"}\n");
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&output)
                .expect("credential metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let credentials: DeviceCredentials =
            serde_json::from_slice(&fs::read(&output).expect("read credential"))
                .expect("decode credential");
        let _authenticated = system.connect(&credentials).await;
        system.stop().await;
    })
    .await
    .expect("enrollment executable test timed out");
}

#[cfg(unix)]
#[tokio::test]
async fn service_executable_runs_alpha_and_stops_cleanly_on_sigterm() {
    timeout(Duration::from_secs(15), async {
        let system = TestSystem::start().await;
        let fixture = HostFixture::install(&system);
        let config = system.files.path().join("node.json");
        let credentials = system.files.path().join("device.json");
        let state = system.files.path().join("node-state");
        let model_bridge = system.files.path().join("model-bridge.mjs");
        let model_credentials = system.files.path().join("credentials.sqlite3");
        write_private(
            &config,
            &serde_json::to_vec(&json!({
                "schemaVersion": 1,
                "endpoint": system.url,
                "model": {
                    "bridge": model_bridge,
                    "credentialStore": model_credentials,
                    "providers": ["xai"],
                    "defaultProvider": "xai",
                    "defaultModel": "fixture-model"
                },
                "targets": [{
                    "target": system.target.as_str(),
                    "profile": renoa_local::ALPHA_PROFILE_ID,
                    "sessionId": fixture.session_id,
                    "workspace": fixture.workspace
                }]
            }))
            .expect("encode node config"),
        );
        write_private(
            &credentials,
            &serde_json::to_vec(&system.enroll_node().await).expect("encode credentials"),
        );
        let process = StdCommand::new(env!("CARGO_BIN_EXE_renoa-node"))
            .args(["serve"])
            .arg(&config)
            .arg(&credentials)
            .arg(&state)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start node service");

        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        let command_id = CommandId::new();
        submit_when_node_is_online(&mut surface, system.task_id, command_id, "Read proof.").await;
        let events = collect_through_terminal(&mut surface).await;
        assert!(events.iter().any(|event| matches!(
            &event.kind,
            renoa_control::TaskEventKind::ExecutionEvent { event, .. }
                if matches!(&event.kind, ExecutionEventKind::AssistantMessage { text }
                    if text == "The durable proof was read.")
        )));
        assert!(matches!(
            events.last().map(|event| &event.kind),
            Some(renoa_control::TaskEventKind::ExecutionEvent { event, .. })
                if matches!(event.kind, ExecutionEventKind::ExecutionTerminated {
                    terminal: ExecutionTerminal::Completed
                })
        ));

        let pid = i32::try_from(process.id()).expect("process id fits i32");
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGTERM,
        )
        .expect("terminate service");
        let output = tokio::task::spawn_blocking(move || process.wait_with_output())
            .await
            .expect("join service wait")
            .expect("wait for service");
        assert!(output.status.success());
        let logs = String::from_utf8(output.stderr).expect("UTF-8 service logs");
        assert!(
            logs.lines()
                .any(|line| line.contains("\"event\":\"service_started\""))
        );
        assert!(
            logs.lines()
                .any(|line| line.contains("\"event\":\"service_stopped\""))
        );
        system.stop().await;
    })
    .await
    .expect("node service executable test timed out");
}

fn write_private(path: &std::path::Path, contents: &[u8]) {
    fs::write(path, contents).expect("write private fixture");
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("protect private fixture");
}

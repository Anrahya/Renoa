use std::{fs, time::Duration};

use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use super::{
    McpAdapterError, McpHostError, discover, discover_cancellable, parse_discovery_record,
};
use crate::mcp::{McpFailureKind, McpOutcomeCertainty, McpRequestHeaders};

#[test]
fn typed_remote_failure_survives_the_process_boundary() {
    let parsed = parse_discovery_record(
        br#"{"wire_version":5,"event":"failed","failure":{"kind":"incompatible_protocol","certainty":"definite","message":"wrong revision","partial_changes_possible":false,"diagnostic":{"code":"protocol_version_mismatch","http_status":409,"detail":"server omitted the pinned revision"}}}
"#,
    )
    .expect("valid terminal record");
    let Err(McpAdapterError::Remote(failure)) = parsed else {
        panic!("expected typed remote failure");
    };

    assert_eq!(failure.kind(), McpFailureKind::IncompatibleProtocol);
    assert_eq!(failure.certainty(), McpOutcomeCertainty::Definite);
    assert!(!failure.partial_changes_possible());
    assert_eq!(failure.diagnostic_code(), Some("protocol_version_mismatch"));
    assert_eq!(failure.diagnostic_http_status(), Some(409));
}

#[test]
fn unknown_failure_class_is_not_accepted_as_a_current_wire_record() {
    let error = parse_discovery_record(
        br#"{"wire_version":5,"event":"failed","failure":{"kind":"maybe","certainty":"definite","message":"ambiguous","partial_changes_possible":false,"diagnostic":{"detail":"invalid class"}}}
"#,
    )
    .expect_err("current wire failure classes are closed");

    assert!(error.contains("decode failed record"));
}

#[test]
fn discovery_accepts_exactly_one_terminal_record() {
    let record = br#"{"wire_version":5,"event":"failed","failure":{"kind":"protocol","certainty":"definite","message":"bad","partial_changes_possible":false,"diagnostic":{"detail":"bad"}}}
"#;
    let mut duplicated = record.to_vec();
    duplicated.extend_from_slice(record);

    assert_eq!(
        parse_discovery_record(&duplicated).expect_err("duplicate terminal record"),
        "adapter returned more than one discovery record"
    );
}

#[tokio::test]
async fn a_valid_terminal_catalog_survives_hung_adapter_cleanup() {
    let directory = tempdir().expect("temporary adapter directory");
    let adapter = directory.path().join("adapter.mjs");
    fs::write(
        &adapter,
        r#"
const terminal = {
  wire_version: 5,
  event: "discovered",
  catalog: {
    endpoint: "https://example.com/mcp",
    protocol_version: "2026-07-28",
    adapter_revision: "mcp-client-node-v0.5.0",
    tools: [],
    rejected_tools: []
  }
};
process.stdout.write(`${JSON.stringify(terminal)}\n`);
await new Promise(() => {});
"#,
    )
    .expect("write hanging adapter");

    let snapshot = tokio::time::timeout(
        Duration::from_secs(3),
        discover(
            &adapter,
            "primary",
            "https://example.com/mcp",
            &McpRequestHeaders::default(),
            None,
        ),
    )
    .await
    .expect("terminal should stop hung cleanup promptly")
    .expect("preserve valid terminal catalog");

    assert_eq!(snapshot.connection_id(), "primary");
    assert!(snapshot.tools().is_empty());
}

#[tokio::test]
async fn records_after_a_terminal_are_rejected_at_the_real_process_boundary() {
    let directory = tempdir().expect("temporary adapter directory");
    let adapter = directory.path().join("adapter.mjs");
    fs::write(
        &adapter,
        r#"
const terminal = {
  wire_version: 5,
  event: "discovered",
  catalog: {
    endpoint: "https://example.com/mcp",
    protocol_version: "2026-07-28",
    adapter_revision: "mcp-client-node-v0.5.0",
    tools: [],
    rejected_tools: []
  }
};
const line = JSON.stringify(terminal);
process.stdout.write(`${line}\n${line}\n`);
await new Promise(() => {});
"#,
    )
    .expect("write duplicate-terminal adapter");

    let error = tokio::time::timeout(
        Duration::from_secs(3),
        discover(
            &adapter,
            "primary",
            "https://example.com/mcp",
            &McpRequestHeaders::default(),
            None,
        ),
    )
    .await
    .expect("duplicate terminal should stop the process promptly")
    .expect_err("duplicate terminal must fail");
    let McpHostError::Adapter(McpAdapterError::Protocol(message)) = error else {
        panic!("expected process protocol failure");
    };

    assert!(message.contains("more than one discovery record"));
}

#[tokio::test]
async fn cancellation_stops_a_hung_discovery_adapter() {
    let directory = tempdir().expect("temporary adapter directory");
    let adapter = directory.path().join("adapter.mjs");
    let started = directory.path().join("started");
    fs::write(
        &adapter,
        format!(
            "import fs from 'node:fs';\nfs.writeFileSync({}, String(process.pid));\nsetInterval(() => {{}}, 60_000);\n",
            serde_json::to_string(&started.to_string_lossy()).expect("encode marker path")
        ),
    )
    .expect("write hanging adapter");
    let cancellation = CancellationToken::new();
    let running_cancellation = cancellation.clone();
    let running = tokio::spawn(async move {
        discover_cancellable(
            &adapter,
            "primary",
            "https://example.com/mcp",
            &McpRequestHeaders::default(),
            None,
            running_cancellation,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("discovery adapter did not start");

    cancellation.cancel();
    let result = tokio::time::timeout(Duration::from_secs(3), running)
        .await
        .expect("discovery cancellation must settle promptly")
        .expect("join discovery task");

    assert!(
        matches!(
            &result,
            Err(McpHostError::Adapter(McpAdapterError::Cancelled))
        ),
        "unexpected discovery cancellation result: {result:?}"
    );
}

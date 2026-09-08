use super::*;
use std::os::unix::fs::PermissionsExt as _;

#[tokio::test]
async fn oversized_process_output_terminates_without_waiting_for_the_writer() {
    for redirect in ["", " >&2"] {
        let mut command = Command::new("sh");
        command.args(["-c", &format!("exec yes output{redirect}")]);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            checked_output(command, &[], &CancellationToken::new()),
        )
        .await
        .expect("oversized writer is terminated");
        assert!(
            result
                .expect_err("reject oversized output")
                .to_string()
                .contains("transport capacity")
        );
    }
}

#[tokio::test]
#[ignore = "requires a built review tools image and a running Docker-compatible engine"]
async fn inspection_container_uses_existing_tools_and_cleans_up_recovered_runs() {
    let directory = tempfile::tempdir().expect("checkout");
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755))
        .expect("checkout mode");
    std::fs::create_dir(directory.path().join("head")).expect("head");
    std::fs::write(
        directory.path().join("head/example.txt"),
        format!("{}last evidence\n", "old source line\n".repeat(10_000)),
    )
    .expect("large source");
    std::os::unix::fs::symlink("/etc/passwd", directory.path().join("head/outside"))
        .expect("escape fixture");
    let config = InspectionContainerConfig {
        engine: std::env::var_os("RENOA_TEST_CONTAINER_ENGINE")
            .map_or_else(|| PathBuf::from("/usr/bin/podman"), PathBuf::from),
        image: "localhost/renoa-review-tools:v1".to_owned(),
    };
    let id = Uuid::new_v4();
    let cancel = CancellationToken::new();
    let first = InspectionContainer::start(&config, id, directory.path(), &cancel)
        .await
        .expect("start container");
    assert_eq!(
        first
            .specs
            .iter()
            .map(|spec| spec.name.as_str())
            .collect::<Vec<_>>(),
        ["read_file", "grep", "find"]
    );
    let mut call = ToolCall {
        id: "read".to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({"path":"head/example.txt","offset":10001,"limit":1}),
        thought_signature: None,
        namespace: None,
    };
    let read = async |container: &InspectionContainer, call: &ToolCall| {
        let bytes = checked_output(
            container.command(),
            &serde_json::to_vec(call).expect("call"),
            &cancel,
        )
        .await
        .expect("worker");
        serde_json::from_slice::<ToolResult>(&bytes).expect("result")
    };
    let result = read(&first, &call).await;
    assert!(!result.is_error);
    assert!(
        serde_json::to_string(&result)
            .expect("result")
            .contains("last evidence")
    );
    call.arguments = serde_json::json!({"path":"head/outside"});
    assert!(read(&first, &call).await.is_error);
    call.name = "bash".to_owned();
    call.arguments = serde_json::json!({"command":"touch /workspace/changed"});
    assert!(read(&first, &call).await.is_error);
    assert!(!directory.path().join("changed").exists());
    // Recreate the same run after a simulated owner crash; never duplicate it.
    let recovered = InspectionContainer::start(&config, id, directory.path(), &cancel)
        .await
        .expect("recover container");
    assert_eq!(first.image, recovered.image);
    recovered.remove().await.expect("cleanup");
    recovered.remove().await.expect("idempotent cleanup");
    let mut inspect = Command::new(&config.engine);
    inspect.args([
        "container",
        "ls",
        "--all",
        "--filter",
        &format!("name={}", recovered.name),
        "--format",
        "{{.Names}}",
    ]);
    assert!(
        checked_output(inspect, &[], &cancel)
            .await
            .expect("inspect cleanup")
            .is_empty()
    );
}

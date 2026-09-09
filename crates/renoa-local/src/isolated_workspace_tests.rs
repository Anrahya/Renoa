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
#[ignore = "requires Bubblewrap >=0.12, ripgrep and a built renoa-workspace-tool"]
async fn inspection_sandbox_uses_existing_tools_without_persistent_processes() {
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
    let config = InspectionSandboxConfig {
        bubblewrap: PathBuf::from("/usr/bin/bwrap"),
        worker: std::env::var_os("RENOA_TEST_INSPECTION_WORKER").map_or_else(
            || {
                std::env::current_dir()
                    .expect("cwd")
                    .join("../../target/debug/renoa-workspace-tool")
            },
            PathBuf::from,
        ),
    };
    let id = Uuid::new_v4();
    let cancel = CancellationToken::new();
    let first = InspectionSandbox::start(&config, id, directory.path(), &cancel)
        .await
        .expect("start container");
    assert_eq!(
        first
            .specs
            .iter()
            .map(|spec| spec.name.as_str())
            .collect::<Vec<_>>(),
        [
            "read_file",
            "grep",
            "find",
            "git_changes",
            "git_diff",
            "git_show"
        ]
    );
    let mut call = ToolCall {
        id: "read".to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({"path":"head/example.txt","offset":10001,"limit":1}),
        thought_signature: None,
        namespace: None,
    };
    let read = async |container: &InspectionSandbox, call: &ToolCall| {
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
    let recovered = InspectionSandbox::start(&config, id, directory.path(), &cancel)
        .await
        .expect("recover container");
    assert_eq!(first.identity, recovered.identity);
}

#[tokio::test]
#[ignore = "requires Bubblewrap >=0.12, Git and a built renoa-workspace-tool"]
async fn git_tools_read_pinned_objects_inside_real_inspection_sandbox() {
    let (directory, _, base, head) = crate::git_repository::tests::fixture();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755))
        .expect("mount mode");
    let config = InspectionSandboxConfig {
        bubblewrap: PathBuf::from("/usr/bin/bwrap"),
        worker: std::env::var_os("RENOA_TEST_INSPECTION_WORKER").map_or_else(
            || {
                std::env::current_dir()
                    .expect("cwd")
                    .join("../../target/debug/renoa-workspace-tool")
            },
            PathBuf::from,
        ),
    };
    let cancel = CancellationToken::new();
    let container = Arc::new(
        InspectionSandbox::start(&config, Uuid::new_v4(), directory.path(), &cancel)
            .await
            .expect("sandbox"),
    );
    let selected = ["git_show".to_owned()].into_iter().collect();
    assert_eq!(container.bindings(Some(&selected)).len(), 1);
    for (name, arguments, expected) in [
        (
            "git_changes",
            serde_json::json!({"base":base,"head":head}),
            "f0000-",
        ),
        (
            "git_diff",
            serde_json::json!({"base":base,"head":head,"path":"removed.rs"}),
            "-check_owner();",
        ),
        (
            "git_show",
            serde_json::json!({"commit":head,"path":"z-bug.rs"}),
            "10 / count",
        ),
    ] {
        let call = ToolCall {
            id: name.to_owned(),
            name: name.to_owned(),
            arguments,
            thought_signature: None,
            namespace: None,
        };
        let bytes = checked_output(
            container.command(),
            &serde_json::to_vec(&call).expect("call"),
            &cancel,
        )
        .await
        .expect("worker");
        let result: ToolResult = serde_json::from_slice(&bytes).expect("result");
        assert!(!result.is_error, "{result:?}");
        assert!(
            serde_json::to_string(&result)
                .expect("text")
                .contains(expected)
        );
    }
}

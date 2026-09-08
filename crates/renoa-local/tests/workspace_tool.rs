use std::{
    io::Write as _,
    process::{Command, Stdio},
};

#[test]
fn workspace_worker_exposes_inspection_only_and_pages_large_files() {
    let directory = tempfile::tempdir().expect("workspace");
    std::fs::write(
        directory.path().join("large.txt"),
        format!("{}evidence\n", "line\n".repeat(20_000)),
    )
    .expect("large file");
    let invoke = |input: &str| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_renoa-workspace-tool"))
            .arg(directory.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("worker");
        child
            .stdin
            .take()
            .expect("input")
            .write_all(input.as_bytes())
            .expect("write input");
        let output = child.wait_with_output().expect("worker exit");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).expect("JSON result")
    };
    let specs = invoke("");
    assert_eq!(
        specs
            .as_array()
            .expect("specs")
            .iter()
            .map(|spec| spec["name"].as_str().expect("name"))
            .collect::<Vec<_>>(),
        ["read_file", "grep", "find"]
    );
    let output = invoke(
        r#"{"id":"read","name":"read_file","arguments":{"path":"large.txt","offset":20001,"limit":1}}"#,
    );
    assert_eq!(output["is_error"], false);
    assert!(output.to_string().contains("evidence"));
    assert_eq!(
        invoke(r#"{"id":"shell","name":"bash","arguments":{"command":"touch changed"}}"#)["is_error"],
        true
    );
    assert_eq!(
        invoke(r#"{"id":"escape","name":"read_file","arguments":{"path":"../outside"}}"#)["is_error"],
        true
    );
    assert!(!directory.path().join("changed").exists());
    std::fs::create_dir_all(directory.path().join(".github/workflows")).expect("hidden directory");
    std::fs::write(
        directory.path().join(".github/workflows/ci.yml"),
        "workflow_evidence\n",
    )
    .expect("workflow");
    let found = invoke(
        r#"{"id":"find-hidden","name":"find","arguments":{"pattern":"**/*.yml","include_hidden":true}}"#,
    );
    assert_eq!(found["is_error"], false);
    assert!(found.to_string().contains(".github/workflows/ci.yml"));
    let found = invoke(
        r#"{"id":"grep-hidden","name":"grep","arguments":{"path":".github","pattern":"workflow_evidence","include_hidden":true}}"#,
    );
    assert_eq!(found["is_error"], false);
    assert!(found.to_string().contains("workflow_evidence"));
    assert_eq!(
        invoke(
            r#"{"id":"escape-hidden","name":"grep","arguments":{"path":"../outside","pattern":"x","include_hidden":true}}"#
        )["is_error"],
        true
    );
}

use std::{fs, path::Path, sync::Arc, time::Duration};

use renoa_agent::{ToolCall, ToolResult, invoke_tool};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

use crate::bash::Bash;

fn call(arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "bash-background".to_owned(),
        name: "bash".to_owned(),
        arguments,
        thought_signature: None,
        namespace: None,
    }
}

async fn run(root: &Path, arguments: serde_json::Value) -> ToolResult {
    let bash = Bash::new(Arc::new(root.to_path_buf()));
    invoke_tool(Some(&bash), call(arguments), CancellationToken::new(), None)
        .await
        .expect("a definite result")
}

fn log(root: &Path, result: &ToolResult) -> std::path::PathBuf {
    let details = result.details.as_ref().expect("details");
    root.join(details["log"].as_str().expect("log path"))
}

/// Waits for `path` to hold `text`, for at most ten seconds.
async fn until_logged(path: &Path, text: &str) -> String {
    for _ in 0..200 {
        let logged = fs::read_to_string(path).unwrap_or_default();
        if logged.contains(text) {
            return logged;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!(
        "{} never held {text:?}: {:?}",
        path.display(),
        fs::read_to_string(path)
    );
}

#[tokio::test]
async fn a_background_command_returns_at_once_and_logs_how_it_ends() {
    let directory = tempdir().expect("temporary directory");

    let result = run(
        directory.path(),
        serde_json::json!({
            "command": "printf started; sleep 1; printf ' rendered'; exit 3",
            "background": true
        }),
    )
    .await;

    assert!(!result.is_error, "{result:?}");
    let log = log(directory.path(), &result);
    assert!(
        !fs::read_to_string(&log)
            .unwrap_or_default()
            .contains("rendered"),
        "the call returned before the command finished"
    );
    let logged = until_logged(&log, "[the command exited with code 3]").await;
    assert!(logged.starts_with("started rendered"), "{logged:?}");
}

#[tokio::test]
async fn a_background_command_is_stopped_at_its_timeout() {
    let directory = tempdir().expect("temporary directory");

    let result = run(
        directory.path(),
        serde_json::json!({
            "command": "sleep 30 & sleep 30",
            "background": true,
            "timeout_seconds": 1
        }),
    )
    .await;

    until_logged(
        &log(directory.path(), &result),
        "[the Host stopped the command after 1 seconds]",
    )
    .await;
}

#[test]
fn a_background_command_does_not_outlive_the_host() {
    let directory = tempdir().expect("temporary directory");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let ticks = directory.path().join("ticks");
    runtime.block_on(async {
        run(
            directory.path(),
            serde_json::json!({
                "command": "while true; do printf . >> ticks; sleep 0.05; done & wait",
                "background": true
            }),
        )
        .await;
        for _ in 0..200 {
            if fs::metadata(&ticks).is_ok_and(|ticks| ticks.len() > 0) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the background command never ran");
    });

    drop(runtime);
    std::thread::sleep(Duration::from_millis(200));
    let stopped = fs::metadata(&ticks).expect("ticks").len();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        fs::metadata(&ticks).expect("ticks").len(),
        stopped,
        "the job's whole process group stopped with the Host"
    );
}

#[tokio::test]
async fn only_a_background_command_runs_longer_than_the_foreground_limit() {
    let directory = tempdir().expect("temporary directory");

    let refused = run(
        directory.path(),
        serde_json::json!({ "command": "true", "timeout_seconds": 1801 }),
    )
    .await;
    assert!(refused.is_error);

    let started = run(
        directory.path(),
        serde_json::json!({ "command": "true", "timeout_seconds": 21600, "background": true }),
    )
    .await;
    assert!(!started.is_error, "{started:?}");
    let too_long = run(
        directory.path(),
        serde_json::json!({ "command": "true", "timeout_seconds": 21601, "background": true }),
    )
    .await;
    assert!(too_long.is_error);
}

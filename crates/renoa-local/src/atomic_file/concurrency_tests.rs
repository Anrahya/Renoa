use std::{io::Write as _, path::Path, sync::Arc};

use renoa_agent::{ToolCall, ToolErrorCode, invoke_tool};
use serde_json::json;
use tempfile::tempdir;
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    sync::Notify,
};
use tokio_util::sync::CancellationToken;

use crate::{
    file_lock::{FileUpdate, probes},
    file_tools::EditFile,
};

async fn edit(
    root: &Path,
    replacement: &str,
    cancellation: CancellationToken,
) -> Result<(), ToolErrorCode> {
    let tool = EditFile::new(Arc::new(root.to_path_buf()));
    let result = invoke_tool(Some(&tool), ToolCall {
        id: replacement.to_owned(), name: "edit_file".to_owned(),
        arguments: json!({"path": "target.txt", "old_text": "original", "new_text": replacement}),
        thought_signature: None, namespace: None,
    }, cancellation, None).await.expect("definite tool result");
    if result.is_error {
        Err(
            serde_json::from_value(result.details.expect("error details")["error"]["code"].clone())
                .expect("error code"),
        )
    } else {
        Ok(())
    }
}

#[tokio::test]
async fn real_edits_from_one_revision_have_one_winner() {
    let directory = tempdir().expect("workspace");
    std::fs::write(directory.path().join("target.txt"), "original").expect("seed file");
    let checked = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let contended = Arc::new(Notify::new());
    let first = probes::CHECKED.scope(
        (Arc::clone(&checked), Arc::clone(&release)),
        edit(directory.path(), "first", CancellationToken::new()),
    );
    let second = async {
        checked.notified().await;
        let update = probes::CONTENDED.scope(
            Arc::clone(&contended),
            edit(directory.path(), "second", CancellationToken::new()),
        );
        tokio::pin!(update);
        tokio::select! {
            result = &mut update => { release.notify_one(); result },
            () = contended.notified() => {
                release.notify_one();
                update.await
            }
        }
    };
    let (first, second) = tokio::join!(first, second);
    first.expect("first edit commits");
    assert_eq!(
        second.expect_err("second edit conflicts"),
        ToolErrorCode::Conflict
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("target.txt")).expect("final file"),
        "first"
    );
}

#[tokio::test]
async fn waiting_for_another_writer_is_cancellable() {
    let directory = tempdir().expect("workspace");
    let path = directory.path().join("target.txt");
    std::fs::write(&path, "original").expect("seed file");
    let owner = FileUpdate::acquire(&path, &CancellationToken::new())
        .await
        .expect("owner");
    let contended = Arc::new(Notify::new());
    let cancellation = CancellationToken::new();
    let update = probes::CONTENDED.scope(
        Arc::clone(&contended),
        edit(directory.path(), "cancelled", cancellation.clone()),
    );
    let cancel = async {
        contended.notified().await;
        cancellation.cancel();
    };
    let (result, ()) = tokio::join!(update, cancel);
    assert_eq!(result.expect_err("cancel waiter"), ToolErrorCode::Cancelled);
    assert_eq!(
        std::fs::read_to_string(&path).expect("unchanged"),
        "original"
    );
    drop(owner);
    edit(directory.path(), "later", CancellationToken::new())
        .await
        .expect("next writer");
}

#[tokio::test]
async fn separate_process_edit_waits_for_check_and_rename_owner() {
    let directory = tempdir().expect("workspace");
    std::fs::write(directory.path().join("target.txt"), "original").expect("seed file");
    let checked = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let first = probes::CHECKED.scope(
        (Arc::clone(&checked), Arc::clone(&release)),
        edit(directory.path(), "first", CancellationToken::new()),
    );
    let child = async {
        checked.notified().await;
        let mut child =
            tokio::process::Command::new(std::env::current_exe().expect("test executable"))
                .args([
                    "--exact",
                    "atomic_file::concurrency_tests::process_edit_writer",
                    "--nocapture",
                ])
                .env("RENOA_TEST_EDIT_ROOT", directory.path())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .expect("spawn competing writer");
        let mut lines = BufReader::new(child.stdout.take().expect("child stdout")).lines();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let line = lines
                    .next_line()
                    .await
                    .expect("child output")
                    .expect("child reached contention");
                if line == "RENOA_EDIT_CONTENDED" {
                    break;
                }
            }
        })
        .await
        .expect("child lock boundary");
        release.notify_one();
        let output = child
            .wait_with_output()
            .await
            .expect("join competing writer");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    let (result, ()) = tokio::join!(first, child);
    result.expect("first edit");
    assert_eq!(
        std::fs::read_to_string(directory.path().join("target.txt")).expect("final file"),
        "first"
    );
}

#[tokio::test]
async fn process_edit_writer() {
    let Some(root) = std::env::var_os("RENOA_TEST_EDIT_ROOT") else {
        return;
    };
    let contended = Arc::new(Notify::new());
    let update = probes::CONTENDED.scope(
        Arc::clone(&contended),
        edit(Path::new(&root), "second", CancellationToken::new()),
    );
    let report = async {
        contended.notified().await;
        println!("RENOA_EDIT_CONTENDED");
        std::io::stdout().flush().expect("flush lock boundary");
    };
    let (result, ()) = tokio::join!(update, report);
    assert_eq!(
        result.expect_err("cross-process stale revision conflicts"),
        ToolErrorCode::Conflict
    );
}

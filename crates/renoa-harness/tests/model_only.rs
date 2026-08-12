use std::{
    env,
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    num::NonZeroU32,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

use futures_util::stream;
use renoa_agent::{
    AssistantContent, AssistantMetadata, ContentBlock, Message, Model, ModelEvent,
    ModelEventStream, ModelRequest, ModelResponse, StopReason, TokenUsage,
};
use renoa_harness::{
    Harness, HarnessError, OperationOutcome, OperationRequest, OperationStatus, RequestId, RunNext,
    RuntimeProfile, SessionId,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

async fn create_session(harness: &Harness) -> SessionId {
    let session_id = SessionId::new();
    harness
        .create_standalone_session(session_id)
        .await
        .expect("create session");
    session_id
}

#[tokio::test]
async fn admission_survives_a_lost_reply() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    let request_id = RequestId::new();
    let request = OperationRequest::new(request_id, vec![ContentBlock::text("build it")]);

    let admitted = harness
        .admit_standalone(session_id, request.clone())
        .await
        .expect("admit operation");
    drop(harness);

    let harness = Harness::open(&database).expect("reopen harness");
    let retried = harness
        .admit_standalone(session_id, request)
        .await
        .expect("retry admission");
    assert_eq!(retried, admitted);

    let conflict = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(request_id, vec![ContentBlock::text("do something else")]),
        )
        .await
        .expect_err("changed content must conflict");
    assert!(matches!(
        conflict,
        HarnessError::RequestConflict {
            request_id: conflicted,
            operation_id,
        } if conflicted == request_id && operation_id == admitted.operation_id
    ));

    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert!(snapshot.messages.is_empty());
    assert_eq!(snapshot.operations.len(), 1);
    assert_eq!(snapshot.operations[0].operation_id, admitted.operation_id);
    assert_eq!(snapshot.operations[0].position, 0);
    assert_eq!(snapshot.operations[0].status, OperationStatus::Queued);
}

#[tokio::test]
async fn one_model_only_operation_completes_durably() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let model = Arc::new(CompletingModel::new(ModelResponse {
        content: vec![AssistantContent::text("done")],
        stop_reason: StopReason::Stop,
        usage: Some(TokenUsage {
            input: 7,
            output: 2,
            cache_read: 0,
            cache_write: 0,
        }),
        metadata: AssistantMetadata::default(),
    }));
    let profile = RuntimeProfile::new(
        "coding-v1",
        model.clone(),
        "Be precise.",
        NonZeroU32::new(2).expect("non-zero attempt limit"),
    );
    let harness = Harness::open(&database).expect("open harness");
    let session_id = create_session(&harness).await;
    let admission = harness
        .admit_standalone(
            session_id,
            OperationRequest::new(RequestId::new(), vec![ContentBlock::text("fix it")]),
        )
        .await
        .expect("admit operation");

    let result = harness
        .run_next(session_id, &profile)
        .await
        .expect("run operation");
    assert_eq!(
        result,
        RunNext::Finished {
            operation_id: admission.operation_id,
            outcome: OperationOutcome::Completed {
                output: "done".to_owned(),
                stop_reason: StopReason::Stop,
                usage: Some(TokenUsage {
                    input: 7,
                    output: 2,
                    cache_read: 0,
                    cache_write: 0,
                }),
            },
        }
    );
    assert_eq!(
        model.requests(),
        vec![ModelRequest {
            system_prompt: "Be precise.".to_owned(),
            messages: vec![Message::user_text("fix it")],
            tools: Vec::new(),
        }]
    );

    drop(harness);
    let harness = Harness::open(&database).expect("reopen harness");
    let snapshot = harness.inspect(session_id).await.expect("inspect session");
    assert_eq!(
        snapshot.messages,
        vec![
            Message::user_text("fix it"),
            Message::Assistant {
                content: vec![AssistantContent::text("done")],
                stop_reason: StopReason::Stop,
                usage: Some(TokenUsage {
                    input: 7,
                    output: 2,
                    cache_read: 0,
                    cache_write: 0,
                }),
                metadata: AssistantMetadata::default(),
            },
        ]
    );
    assert_eq!(snapshot.operations[0].status, OperationStatus::Completed);
}

#[cfg(unix)]
#[test]
fn a_symlink_alias_cannot_bypass_the_live_owner() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let alias = directory.path().join("alias.sqlite3");
    let _harness = Harness::open(&database).expect("open harness");
    symlink(&database, &alias).expect("create database symlink");

    assert_eq!(
        Harness::open(&alias)
            .err()
            .expect("alias must share the original lock"),
        HarnessError::AlreadyRunning {
            path: database.canonicalize().expect("canonical database path"),
        }
    );
}

#[test]
fn a_second_harness_in_the_same_process_cannot_own_the_database() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let _harness = Harness::open(&database).expect("open harness");

    assert_eq!(
        Harness::open(&database)
            .err()
            .expect("second harness must not own the database"),
        HarnessError::AlreadyRunning {
            path: database.canonicalize().expect("canonical database path"),
        }
    );
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_and_its_target_share_one_owner() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().expect("temporary directory");
    let target = directory.path().join("missing.sqlite3");
    let alias = directory.path().join("alias.sqlite3");
    symlink(&target, &alias).expect("create dangling symlink");
    let _harness = Harness::open(&alias).expect("open through dangling alias");

    assert!(
        target.exists(),
        "opening the alias creates its database target"
    );
    assert_eq!(
        Harness::open(&target)
            .err()
            .expect("target must share the alias owner"),
        HarnessError::AlreadyRunning {
            path: target.canonicalize().expect("canonical target path"),
        }
    );
}

#[cfg(unix)]
#[test]
fn a_hard_link_database_alias_is_rejected() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let alias = directory.path().join("alias.sqlite3");
    drop(Harness::open(&database).expect("create harness database"));
    fs::hard_link(&database, &alias).expect("create database hard link");

    assert_eq!(
        Harness::open(&alias)
            .err()
            .expect("hard links must be rejected"),
        HarnessError::UnsupportedDatabaseAlias {
            path: alias.canonicalize().expect("canonical alias path"),
        }
    );
}

#[tokio::test]
async fn replacing_the_live_lock_path_fails_closed() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Harness::open(&database).expect("open harness");
    let lock = sibling_lock_path(&database.canonicalize().expect("canonical database path"));
    fs::remove_file(&lock).expect("unlink lock path");
    File::create(&lock).expect("replace lock path");

    let error = harness
        .create_standalone_session(SessionId::new())
        .await
        .expect_err("replaced lock must stop writes");
    assert!(
        matches!(error, HarnessError::Store(message) if message.contains("changed while the harness owned it"))
    );
}

#[tokio::test]
async fn replacing_the_live_database_path_fails_closed() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let harness = Harness::open(&database).expect("open harness");
    fs::remove_file(&database).expect("unlink database path");
    File::create(&database).expect("replace database path");

    let error = harness
        .create_standalone_session(SessionId::new())
        .await
        .expect_err("replaced database must stop writes");
    assert!(matches!(
        error,
        HarnessError::UnsupportedDatabaseAlias { .. } | HarnessError::Store(_)
    ));
}

#[test]
fn a_second_process_cannot_open_the_live_database() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("harness.sqlite3");
    let mut child = Command::new(env::current_exe().expect("current test binary"))
        .args(["--exact", "lock_holder_process", "--ignored", "--nocapture"])
        .env("RENOA_HARNESS_LOCK_TEST_DB", &database)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn lock holder");
    let mut output = BufReader::new(child.stdout.take().expect("child stdout"));
    let mut line = String::new();
    loop {
        let bytes = output.read_line(&mut line).expect("read ready signal");
        assert_ne!(bytes, 0, "lock holder exited before becoming ready");
        if line.trim() == "READY" {
            break;
        }
        line.clear();
    }

    let error = Harness::open(&database).err().expect("lock must be held");
    assert_eq!(
        error,
        HarnessError::AlreadyRunning {
            path: database.canonicalize().expect("canonical database path")
        }
    );

    drop(child.stdin.take());
    assert!(child.wait().expect("wait for lock holder").success());
    Harness::open(&database).expect("lock released after owner exits");
}

#[test]
#[ignore = "helper process for a_second_process_cannot_open_the_live_database"]
fn lock_holder_process() {
    let database = env::var_os("RENOA_HARNESS_LOCK_TEST_DB").expect("lock-test database path");
    let _harness = Harness::open(database).expect("lock database");
    println!("READY");
    std::io::stdin()
        .read_to_end(&mut Vec::new())
        .expect("wait for parent");
}

fn sibling_lock_path(database: &std::path::Path) -> std::path::PathBuf {
    let mut lock = database.as_os_str().to_owned();
    lock.push(".lock");
    lock.into()
}

struct CompletingModel {
    response: ModelResponse,
    requests: Mutex<Vec<ModelRequest>>,
}

impl CompletingModel {
    fn new(response: ModelResponse) -> Self {
        Self {
            response,
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

impl Model for CompletingModel {
    fn stream(
        &self,
        request: ModelRequest,
        _cancellation: CancellationToken,
    ) -> ModelEventStream<'_> {
        self.requests.lock().expect("request lock").push(request);
        Box::pin(stream::once(std::future::ready(Ok(
            ModelEvent::Completed {
                response: self.response.clone(),
            },
        ))))
    }
}

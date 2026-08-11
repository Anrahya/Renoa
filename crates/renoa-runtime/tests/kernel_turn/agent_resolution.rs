use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use crate::support::{ScriptedModel, TestCapabilityHost, test_command, tool_response};
use renoa_core::{
    BoxFuture, CapabilityCall, CapabilityHost, CapabilityOutcome, CapabilityRequest,
    CapabilitySpec, Message, ModelResponse, ResolvedAgent, RunEventKind, RunStore,
};
use renoa_runtime::{Engine, EngineConfig};
use renoa_store_sqlite::SqliteRunStore;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn resolved_agent_shapes_the_model_request() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::new(vec![final_response()]));
    let capabilities = Arc::new(TestCapabilityHost::new(workspace.path()));
    let store = Arc::new(
        SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
    );
    let engine = Engine::new(model.clone(), capabilities, store, EngineConfig::default());
    let agent = read_only_agent("Review the target without modifying it.");

    engine
        .run(test_command(), agent, CancellationToken::new())
        .await
        .expect("resolved agent turn must complete");

    let requests = model.requests();
    assert_eq!(
        requests[0].messages[0],
        Message::System {
            text: "Review the target without modifying it.".to_owned(),
        }
    );
    assert_eq!(
        requests[0]
            .capabilities
            .iter()
            .map(|capability| capability.name.as_str())
            .collect::<Vec<_>>(),
        vec!["read_file"]
    );
}

#[tokio::test]
async fn capability_manifest_is_frozen_for_the_run() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::new(vec![
        tool_response(
            "inspect",
            "read_file",
            serde_json::json!({ "path": "unused" }),
        ),
        final_response(),
    ]));
    let capabilities = Arc::new(ChangingManifestHost::new());
    let store = Arc::new(
        SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
    );
    let engine = Engine::new(
        model.clone(),
        capabilities.clone(),
        store,
        EngineConfig::default(),
    );

    engine
        .run(
            test_command(),
            ResolvedAgent {
                instructions: "Inspect the target.".to_owned(),
                capability_grants: vec!["read_file".to_owned(), "edit_file".to_owned()],
            },
            CancellationToken::new(),
        )
        .await
        .expect("multi-round turn must complete");

    assert_eq!(capabilities.manifest_reads(), 1);
    let requests = model.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].capabilities, requests[1].capabilities);
    assert_eq!(requests[0].capabilities[0].name, "read_file");
}

#[tokio::test]
async fn ungranted_capability_never_reaches_the_host() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::new(vec![
        tool_response(
            "denied-edit",
            "edit_file",
            serde_json::json!({
                "path": "hello.txt",
                "oldText": "before",
                "newText": "after",
            }),
        ),
        final_response(),
    ]));
    let capabilities = Arc::new(TestCapabilityHost::new(workspace.path()));
    let store = Arc::new(
        SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
    );
    let engine = Engine::new(
        model.clone(),
        capabilities.clone(),
        store,
        EngineConfig::default(),
    );

    engine
        .run(
            test_command(),
            read_only_agent("Inspect without modifying the target."),
            CancellationToken::new(),
        )
        .await
        .expect("the model must be able to recover from a denied call");

    assert_eq!(capabilities.executions(), Vec::<String>::new());
    assert!(matches!(
        &model.requests()[1].messages[3],
        Message::Capability { outcome, .. }
            if outcome.is_error
                && outcome.model_view["error"]
                    .as_str()
                    .is_some_and(|message| message.contains("not granted"))
    ));
}

#[tokio::test]
async fn mixed_capability_batch_executes_only_granted_calls_in_source_order() {
    let workspace = tempdir().expect("temporary workspace must be created");
    fs::write(workspace.path().join("hello.txt"), "hello\n").expect("fixture must be written");
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse {
            text: String::new(),
            capability_calls: vec![
                CapabilityCall {
                    call_id: "denied-edit".to_owned(),
                    name: "edit_file".to_owned(),
                    arguments: serde_json::json!({
                        "path": "hello.txt",
                        "oldText": "hello",
                        "newText": "changed",
                    }),
                },
                CapabilityCall {
                    call_id: "allowed-read".to_owned(),
                    name: "read_file".to_owned(),
                    arguments: serde_json::json!({ "path": "hello.txt" }),
                },
            ],
            truncated: false,
        },
        final_response(),
    ]));
    let capabilities = Arc::new(TestCapabilityHost::new(workspace.path()));
    let store = Arc::new(
        SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
    );
    let engine = Engine::new(
        model.clone(),
        capabilities.clone(),
        store,
        EngineConfig::default(),
    );

    engine
        .run(
            test_command(),
            read_only_agent("Read without modifying the target."),
            CancellationToken::new(),
        )
        .await
        .expect("mixed capability turn must complete");

    assert_eq!(capabilities.executions(), vec!["allowed-read"]);
    let requests = model.requests();
    let results = &requests[1].messages[3..];
    assert!(matches!(
        &results[0],
        Message::Capability {
            call_id,
            outcome,
            ..
        } if call_id == "denied-edit" && outcome.is_error
    ));
    assert!(matches!(
        &results[1],
        Message::Capability {
            call_id,
            outcome,
            ..
        } if call_id == "allowed-read"
            && !outcome.is_error
            && outcome.model_view["content"] == "hello\n"
    ));
}

#[tokio::test]
async fn resolved_agent_snapshot_survives_store_reopen() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let database_path = workspace.path().join("renoa.db");
    let model = Arc::new(ScriptedModel::new(vec![final_response()]));
    let capabilities = Arc::new(TestCapabilityHost::new(workspace.path()));
    let store = Arc::new(SqliteRunStore::open(&database_path).expect("run store must open"));
    let engine = Engine::new(model, capabilities, store.clone(), EngineConfig::default());
    let agent = read_only_agent("Act as a read-only reviewer.");

    let result = engine
        .run(test_command(), agent.clone(), CancellationToken::new())
        .await
        .expect("resolved agent turn must complete");

    drop(engine);
    drop(store);
    let reopened_store = SqliteRunStore::open(database_path).expect("run store must reopen");
    let transcript = reopened_store
        .load_transcript(result.run_id)
        .await
        .expect("run transcript must survive reopening");
    assert_eq!(transcript.run.agent, agent);
    assert!(matches!(
        &transcript.events[0].kind,
        RunEventKind::RunStarted {
            agent: persisted,
            ..
        } if persisted == &agent
    ));
}

fn read_only_agent(instructions: &str) -> ResolvedAgent {
    ResolvedAgent {
        instructions: instructions.to_owned(),
        capability_grants: vec!["read_file".to_owned()],
    }
}

fn final_response() -> ModelResponse {
    ModelResponse {
        text: "Done.".to_owned(),
        capability_calls: Vec::new(),
        truncated: false,
    }
}

struct ChangingManifestHost {
    manifest_reads: AtomicUsize,
}

impl ChangingManifestHost {
    fn new() -> Self {
        Self {
            manifest_reads: AtomicUsize::new(0),
        }
    }

    fn manifest_reads(&self) -> usize {
        self.manifest_reads.load(Ordering::SeqCst)
    }
}

impl CapabilityHost for ChangingManifestHost {
    fn specs(&self) -> Vec<CapabilitySpec> {
        let name = if self.manifest_reads.fetch_add(1, Ordering::SeqCst) == 0 {
            "read_file"
        } else {
            "edit_file"
        };
        vec![CapabilitySpec {
            name: name.to_owned(),
            description: name.to_owned(),
            input_schema: serde_json::json!({ "type": "object" }),
        }]
    }

    fn execute(
        &self,
        _request: CapabilityRequest,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, CapabilityOutcome> {
        Box::pin(async {
            CapabilityOutcome {
                model_view: serde_json::json!({ "content": "inspected" }),
                is_error: false,
            }
        })
    }
}

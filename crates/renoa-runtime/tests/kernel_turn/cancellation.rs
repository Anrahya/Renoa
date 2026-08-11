use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::support::{ScriptedModel, event_name, test_command, tool_response};
use renoa_core::{
    BoxFuture, CapabilityHost, CapabilityOutcome, CapabilityRequest, CapabilitySpec, ResolvedAgent,
    RunEventKind, RunStatus, RunStore, TerminalState,
};
use renoa_runtime::{Engine, EngineConfig, EngineError};
use renoa_store_sqlite::SqliteRunStore;
use tempfile::tempdir;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn cancellation_during_capability_execution_is_durable() {
    let workspace = tempdir().expect("temporary workspace must be created");
    let model = Arc::new(ScriptedModel::new(vec![tool_response(
        "pending-call",
        "wait",
        serde_json::json!({}),
    )]));
    let capabilities = Arc::new(PendingCapabilityHost::new());
    let store = Arc::new(
        SqliteRunStore::open(workspace.path().join("renoa.db")).expect("run store must open"),
    );
    let engine = Engine::new(
        model.clone(),
        capabilities.clone(),
        store.clone(),
        EngineConfig::default(),
    );
    let cancellation = CancellationToken::new();
    let canceller = tokio::spawn({
        let capabilities = capabilities.clone();
        let cancellation = cancellation.clone();
        async move {
            capabilities.wait_until_started().await;
            cancellation.cancel();
        }
    });

    let error = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        engine.run(
            test_command(),
            ResolvedAgent {
                instructions: "Wait until cancelled.".to_owned(),
                capability_grants: vec!["wait".to_owned()],
            },
            cancellation,
        ),
    )
    .await
    .expect("capability cancellation must settle the run")
    .expect_err("cancelled run must not complete successfully");
    canceller.await.expect("cancellation task must finish");

    assert!(matches!(error, EngineError::Cancelled));
    assert!(capabilities.execution_was_dropped());
    assert_eq!(model.requests().len(), 1);
    let transcript = store
        .load_transcript(model.requests()[0].run_id)
        .await
        .expect("cancelled transcript must load");
    assert_eq!(transcript.run.status, RunStatus::Cancelled);
    assert_eq!(
        transcript.run.terminal,
        Some(TerminalState::Cancelled {
            reason: "caller cancelled the run".to_owned(),
        })
    );
    assert_eq!(
        transcript
            .events
            .iter()
            .map(|event| event_name(&event.kind))
            .collect::<Vec<_>>(),
        vec![
            "run_started",
            "model_requested",
            "model_responded",
            "capability_requested",
            "capability_completed",
            "run_terminated",
        ]
    );
    assert!(transcript.events.iter().any(|event| matches!(
        &event.kind,
        RunEventKind::CapabilityCompleted { outcome, .. }
            if outcome.is_error
                && outcome.model_view["error"] == "capability execution was cancelled"
    )));
}

struct PendingCapabilityHost {
    started: Semaphore,
    dropped: AtomicBool,
}

impl PendingCapabilityHost {
    fn new() -> Self {
        Self {
            started: Semaphore::new(0),
            dropped: AtomicBool::new(false),
        }
    }

    async fn wait_until_started(&self) {
        self.started
            .acquire()
            .await
            .expect("pending capability semaphore must remain open")
            .forget();
    }

    fn execution_was_dropped(&self) -> bool {
        self.dropped.load(Ordering::SeqCst)
    }
}

impl CapabilityHost for PendingCapabilityHost {
    fn specs(&self) -> Vec<CapabilitySpec> {
        vec![CapabilitySpec {
            name: "wait".to_owned(),
            description: "Wait indefinitely".to_owned(),
            input_schema: serde_json::json!({ "type": "object" }),
        }]
    }

    fn execute(
        &self,
        _request: CapabilityRequest,
        _cancellation: CancellationToken,
    ) -> BoxFuture<'_, CapabilityOutcome> {
        Box::pin(async move {
            let _drop_guard = DropFlag(&self.dropped);
            self.started.add_permits(1);
            std::future::pending().await
        })
    }
}

struct DropFlag<'a>(&'a AtomicBool);

impl Drop for DropFlag<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

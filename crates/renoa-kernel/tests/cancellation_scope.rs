use std::sync::{Arc, Mutex};

use renoa_kernel::{
    AgentId, CancellationId, CancellationInput, CancellationTransition, Checkpoint, Command,
    CommandId, DriveResult, EffectAdapter, EffectBinding, EffectFuture, EffectInvocation,
    EffectOutcome, EffectRecovery, Kernel, KernelError, LoopBinding, LoopDecision, LoopError,
    LoopInput, LoopPlugin, OperationOutcome, Runtime, SessionId,
};
use tempfile::tempdir;
use tokio::sync::{Notify, oneshot};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn retrying_an_old_cancellation_cannot_signal_the_next_live_operation() {
    let directory = tempdir().expect("temporary directory");
    let kernel =
        Arc::new(Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel"));
    let session_id = create_session(&kernel);
    let first = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"mode": "pause"})),
        )
        .expect("submit first command");
    let cancellation_id = CancellationId::new();
    let release = Arc::new(Notify::new());
    let (token_tx, token_rx) = oneshot::channel();
    let runtime = Arc::new(runtime(token_tx, Arc::clone(&release)));

    assert!(matches!(
        kernel.drive(session_id, runtime.as_ref()).await,
        Err(KernelError::Loop(error)) if error.message() == "pause before cancellation"
    ));
    kernel
        .request_cancellation(session_id, first.operation_id, cancellation_id)
        .expect("request first cancellation");
    assert_eq!(
        kernel
            .drive(session_id, runtime.as_ref())
            .await
            .expect("close first cancellation"),
        DriveResult::Finished {
            operation_id: first.operation_id,
            outcome: OperationOutcome::Cancelled,
        }
    );

    let second = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"mode": "effect"})),
        )
        .expect("submit second command");
    let runner = Arc::clone(&kernel);
    let driven_runtime = Arc::clone(&runtime);
    let drive =
        tokio::spawn(async move { runner.drive(session_id, driven_runtime.as_ref()).await });
    let live_token = token_rx.await.expect("receive second operation token");

    kernel
        .request_cancellation(session_id, first.operation_id, cancellation_id)
        .expect("retry old cancellation");
    assert!(!live_token.is_cancelled());
    release.notify_one();
    assert_eq!(
        drive
            .await
            .expect("join second drive")
            .expect("complete second operation"),
        DriveResult::Finished {
            operation_id: second.operation_id,
            outcome: OperationOutcome::Completed,
        }
    );
}

fn create_session(kernel: &Kernel) -> SessionId {
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    session_id
}

fn runtime(token: oneshot::Sender<CancellationToken>, release: Arc<Notify>) -> Runtime {
    Runtime::new(
        LoopBinding::new("scope-loop", "1", Arc::new(ScopeLoop)),
        1,
        "scope-config-1",
        vec![EffectBinding::new(
            "external",
            "1",
            Arc::new(ProbeAdapter {
                token: Mutex::new(Some(token)),
                release,
            }),
        )],
    )
    .expect("valid runtime")
}

struct ScopeLoop;

impl LoopPlugin for ScopeLoop {
    fn decide(&self, input: LoopInput) -> Result<LoopDecision, LoopError> {
        if input.command.content()["mode"] == "pause" {
            return Err(LoopError::new("pause before cancellation"));
        }
        if input.effect.is_some() {
            Ok(LoopDecision::Complete {
                checkpoint: terminal_checkpoint(),
                events: Vec::new(),
            })
        } else {
            Ok(LoopDecision::InvokeEffect {
                checkpoint: Checkpoint::new(1, serde_json::json!({"awaiting": true})),
                binding: "external".to_owned(),
                request: input.command.content().clone(),
                recovery: EffectRecovery::NeverReplay,
            })
        }
    }

    fn cancel_operation(
        &self,
        _input: CancellationInput,
    ) -> Result<CancellationTransition, LoopError> {
        Ok(CancellationTransition {
            checkpoint: terminal_checkpoint(),
            events: Vec::new(),
        })
    }
}

struct ProbeAdapter {
    token: Mutex<Option<oneshot::Sender<CancellationToken>>>,
    release: Arc<Notify>,
}

impl EffectAdapter for ProbeAdapter {
    fn invoke(&self, invocation: EffectInvocation) -> EffectFuture<'_> {
        self.token
            .lock()
            .expect("token sender")
            .take()
            .expect("single probe invocation")
            .send(invocation.cancellation.clone())
            .expect("send cancellation token");
        Box::pin(async move {
            self.release.notified().await;
            EffectOutcome::Success(serde_json::json!(true)).into()
        })
    }
}

fn terminal_checkpoint() -> Checkpoint {
    Checkpoint::new(1, serde_json::json!({"terminal": true}))
}

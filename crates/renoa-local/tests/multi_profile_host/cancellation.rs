use super::*;
use renoa_agent_loop::AgentCommand;
use renoa_kernel::{Command, CommandId, Kernel, OperationStatus, SessionId};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn cancellation_without_a_runtime_keeps_unfinished_work_durable_and_owned() {
    for cached in [false, true] {
        let directory = tempdir().expect("Host root");
        let workspace = directory.path().join("workspace");
        let data = directory.path().join("data");
        let bridge = directory.path().join("bridge.mjs");
        let auth = directory.path().join("auth.sqlite");
        fs::create_dir(&workspace).expect("workspace");
        fs::write(&bridge, MODEL_BRIDGE).expect("model fixture");
        fs::write(&auth, "").expect("credential fixture");
        let host = local_host(&data, &bridge, &auth, true);
        let profile = AgentProfileId::new(RELAY_PROFILE_ID).expect("profile");
        let session = host
            .create_session(&profile, &workspace)
            .await
            .expect("session");
        let id = session.id();
        drop(session);
        let database = data
            .join("sessions")
            .join(id.to_string())
            .join("kernel.sqlite3");
        let request_id = Uuid::new_v4();
        let content = vec![ContentBlock::text("Original")];
        let kernel = Kernel::open(&database).expect("admission owner");
        kernel
            .submit(
                SessionId::from_uuid(id),
                Command::new(
                    CommandId::from_uuid(request_id),
                    serde_json::to_value(AgentCommand::new(content.clone())).expect("command"),
                ),
            )
            .expect("admit before interruption");
        drop(kernel);
        let session = if cached {
            Some(
                host.load_session(id, &workspace)
                    .await
                    .expect("cache executable session"),
            )
        } else {
            None
        };
        fs::remove_file(&bridge).expect("make runtime unavailable");
        let changed = vec![ContentBlock::text("Changed")];
        let conflict = cancel(
            &host,
            session.as_deref(),
            &profile,
            &workspace,
            id,
            request_id,
            &changed,
        )
        .await
        .expect_err("reject changed command");
        assert!(conflict.to_string().contains("different content"));
        assert_eq!(
            cancel(
                &host,
                session.as_deref(),
                &profile,
                &workspace,
                id,
                request_id,
                &content
            )
            .await
            .expect("persist cancellation without runtime"),
            None
        );
        if let Some(session) = &session {
            assert!(
                Kernel::open(&database).is_err(),
                "cached session retains ownership"
            );
            let token = CancellationToken::new();
            token.cancel();
            assert!(
                session
                    .execute_turn_observed_with_cancellation(
                        request_id,
                        content.clone(),
                        renoa_local::TurnObservation::now().expect("time"),
                        Arc::new(NoopEvents),
                        token
                    )
                    .await
                    .is_err(),
                "an unfinished operation needs its real runtime to settle"
            );
        } else {
            assert!(host.load_session(id, &workspace).await.is_err());
        }
        drop(session);
        assert_durable_recovery(
            &host, &database, &bridge, &workspace, id, request_id, content,
        )
        .await;
    }
}

async fn cancel(
    host: &LocalHost,
    session: Option<&renoa_local::AgentSession>,
    profile: &AgentProfileId,
    workspace: &Path,
    id: Uuid,
    request_id: Uuid,
    content: &[ContentBlock],
) -> Result<Option<LocalTurnOutcome>, LocalHostError> {
    if let Some(session) = session {
        session.cancel_before_execution(request_id, Some(content))
    } else {
        host.cancel_before_execution(profile, workspace, id, request_id, Some(content))
            .await
    }
}

async fn assert_durable_recovery(
    host: &LocalHost,
    database: &Path,
    bridge: &Path,
    workspace: &Path,
    id: Uuid,
    request_id: Uuid,
    content: Vec<ContentBlock>,
) {
    let kernel = Kernel::open(database).expect("inspect interruption");
    let snapshot = kernel.inspect(SessionId::from_uuid(id)).expect("snapshot");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Queued);
    assert_eq!(snapshot.operations[0].outcome, None);
    assert!(snapshot.operations[0].effects.is_empty());
    drop(kernel);
    let connection = Connection::open(database).expect("cancellation database");
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM cancellation_requests", [], |row| row
                .get::<_, i64>(
                0
            ))
            .expect("persisted cancellation"),
        1
    );
    drop(connection);
    fs::write(bridge, MODEL_BRIDGE).expect("restore actual runtime");
    let session = host
        .load_session(id, workspace)
        .await
        .expect("recover session");
    assert_eq!(
        session
            .execute_turn(request_id, content, Arc::new(NoopEvents))
            .await
            .expect("settle persisted cancellation"),
        LocalTurnOutcome::Cancelled
    );
    assert!(matches!(
        session
            .execute_turn(
                Uuid::new_v4(),
                vec![ContentBlock::text("Continue")],
                Arc::new(NoopEvents)
            )
            .await
            .expect("later request"),
        LocalTurnOutcome::Completed { .. }
    ));
}

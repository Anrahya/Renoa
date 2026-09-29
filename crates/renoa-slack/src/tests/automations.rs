use super::*;
use renoa_local::AutomationRun;

#[tokio::test]
async fn completed_host_results_wait_for_a_channel_and_deliver_once_without_executing_a_model() {
    let f = Fixture::new().await;
    let run = AutomationRun {
        sequence: 2,
        id: Uuid::new_v4(),
        automation_id: Uuid::new_v4(),
        agent_id: f.worker.agent_id,
        due_ms: 1,
        admitted_at_ms: 1,
        submission: "digest".to_owned(),
        result: Some(renoa_local::RunResult {
            status: renoa_local::RunStatus::Succeeded,
            output: "**Digest**\n- Saved `digest.md`".to_owned(),
            failed_tool_calls: Some(0),
            finished_at_ms: Some(1),
        }),
    };
    let mut blocked = run.clone();
    blocked.sequence = 1;
    blocked.id = Uuid::new_v4();
    blocked.agent_id = renoa_kernel::AgentId::new();
    f.worker
        .store
        .admit_automation_result(blocked)
        .await
        .expect("earlier unbound bot");
    f.worker
        .store
        .admit_automation_result(run.clone())
        .await
        .expect("unbound result retained in outbox");
    assert_eq!(f.worker.store.automation_cursor().await.expect("cursor"), 2);
    let id = run.agent_id.to_string();
    f.worker.store.run(move|db|{db.execute("INSERT INTO bot_channels(agent_id,name,channel_id,state) VALUES(?1,'digest','C1','ready')",[id])?;Ok(())}).await.expect("ready binding");
    f.worker
        .store
        .admit_automation_result(run.clone())
        .await
        .expect("durable outbox");
    assert_eq!(
        f.worker
            .store
            .automation_cursor()
            .await
            .expect("committed cursor"),
        2
    );
    let projector = crate::automations::Automations {
        host: f.worker.host.clone(),
        store: f.worker.store.clone(),
        api: Arc::clone(&f.worker.api),
        shutdown: CancellationToken::new(),
    };
    projector.deliver_one().await.expect("deliver");
    f.worker
        .store
        .admit_automation_result(run)
        .await
        .expect("replay");
    projector.deliver_one().await.expect("no duplicate");
    let sent = f.sent.lock().await;
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["channel"], "C1");
    assert_eq!(
        sent[0]["text"],
        "Automation result\n\n**Digest**\n- Saved `digest.md`"
    );
    assert_eq!(
        sent[0]["blocks"],
        json!([{"type":"markdown","text":"Automation result\n\n**Digest**\n- Saved `digest.md`"}])
    );
    drop(sent);
    drop(projector);
    f.stop().await;
}

mod context;

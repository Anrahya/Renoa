use super::*;

#[tokio::test]
async fn model_receives_admitted_surface_context_without_rewriting_earlier_prompt_prefixes() {
    let mut fixture = Fixture::new().await;
    fixture
        .admit(
            "E1",
            "1.000001",
            "Can you create a bot with its own channel?",
        )
        .await;
    let first = fixture
        .worker
        .store
        .next_work()
        .await
        .expect("queue")
        .expect("first");
    let snapshot = first.surface_context.clone().expect("admitted context");
    fixture.worker.execute(first).await.expect("execute");
    fixture
        .admit("E2", "2.000001", "Are discovery results free?")
        .await;
    // Model execution must use the admitted snapshot, including after a
    // version change, rather than whichever template the binary now embeds.
    fixture.worker.store.run(|db| {
        db.execute("UPDATE requests SET surface_context='previous interface snapshot' WHERE state='queued'",[])?;
        Ok(())
    }).await.expect("older admission snapshot");
    fixture.worker.session = None;
    let second = fixture
        .worker
        .store
        .next_work()
        .await
        .expect("queue")
        .expect("second");
    assert_eq!(
        second.surface_context.as_deref(),
        Some("previous interface snapshot")
    );
    fixture
        .worker
        .execute(second)
        .await
        .expect("reopen and execute");
    let captured = std::fs::read_to_string(fixture.bridge.with_file_name("model-requests"))
        .expect("model boundary");
    let requests: Vec<Value> = captured
        .lines()
        .map(|line| serde_json::from_str(line).expect("request"))
        .collect();
    assert_eq!(requests.len(), 2);
    let first = &requests[0];
    let second = &requests[1];
    assert_eq!(first["system_prompt"], second["system_prompt"]);
    assert_eq!(first["messages"][0], second["messages"][0]);
    let first_context = first["messages"][0]["content"][1]["text"]
        .as_str()
        .expect("context");
    assert_eq!(first_context, snapshot);
    assert!(first_context.contains("active interface is Slack"));
    assert!(first_context.contains("no Slack MCP connection is required"));
    assert!(first_context.contains("still costs tokens"));
    let latest = second["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .rfind(|message| message["role"] == "user")
        .expect("latest");
    assert_eq!(latest["content"][1]["text"], "previous interface snapshot");
    fixture.stop().await;
}

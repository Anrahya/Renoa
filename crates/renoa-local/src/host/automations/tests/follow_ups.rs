use super::super::follow_up;
use super::*;
use renoa_agent::ToolCall;
use renoa_kernel::{CommandId, SessionId};

const NOTE: &str = "Ask whether the table was booked";

/// The agent's conversation and the follow-up its model sets in it, after
/// `minutes`, through the resident tool.
async fn set_by_model(
    h: &LocalHost,
    agent: AgentId,
    session: &AgentSession,
    minutes: u64,
) -> AutomationRecord {
    let output = session
        .execute_turn(
            Uuid::new_v4(),
            vec![ContentBlock::text(format!("follow up {minutes}"))],
            Arc::new(Quiet),
        )
        .await
        .expect("the model sets a follow-up");
    let LocalTurnOutcome::Completed { output, .. } = output else {
        panic!("the turn completes");
    };
    let set: Value = serde_json::from_str(&output).expect("the tool's JSON");
    let id = set["id"].as_str().expect("id").parse().expect("uuid");
    let record = h.automation(agent, id).await.expect("the follow-up");
    assert_eq!(set["revision"], record.revision);
    record
}

/// A conversation of `agent`, and its id.
async fn conversation(h: &LocalHost, agent: AgentId) -> (Arc<AgentSession>, SessionId) {
    let workspace = h.agent_workspace(agent).await.expect("workspace");
    let id = Uuid::new_v4();
    let session = h
        .ensure_agent_session(agent, &workspace, id)
        .await
        .expect("conversation");
    (session, SessionId::from_uuid(id))
}

/// Calls the tool as a turn of `session` would, with call `id`, for a
/// follow-up an hour from now.
async fn call(
    h: &LocalHost,
    agent: AgentId,
    session: SessionId,
    command: CommandId,
    id: &str,
) -> renoa_agent::ToolResult {
    call_after(h, agent, session, command, id, 60).await
}

async fn call_after(
    h: &LocalHost,
    agent: AgentId,
    session: SessionId,
    command: CommandId,
    id: &str,
    minutes: u64,
) -> renoa_agent::ToolResult {
    let binding = follow_up::binding(Arc::clone(&h.config), agent, session, Some(command));
    renoa_agent::invoke_tool(
        Some(binding.tool().as_ref()),
        ToolCall {
            id: id.to_owned(),
            name: "follow_up".to_owned(),
            arguments: json!({"note": NOTE, "after_minutes": minutes}),
            namespace: None,
            thought_signature: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("known outcome")
}

fn text(result: &renoa_agent::ToolResult) -> &str {
    let [ContentBlock::Text { text }] = result.content.as_slice() else {
        panic!("one text block")
    };
    text
}

fn landed(answer: &str, landing: RunLanding) -> RunOutcome {
    RunOutcome::Succeeded {
        answer: answer.to_owned(),
        failed_tool_calls: 0,
        landing,
    }
}

/// A follow-up is a one-time automation of its agent, due `after_minutes`
/// from now, that returns to the conversation it was set in; once its run
/// succeeded there, it is gone.
#[tokio::test]
async fn a_follow_up_returns_to_its_conversation_once_and_is_then_removed() {
    let (_d, h, parent, _) = fixture().await;
    let (chat, chat_id) = conversation(&h, parent).await;
    let before = crate::TurnObservation::now()
        .expect("clock")
        .unix_milliseconds();
    let record = set_by_model(&h, parent, &chat, 1440).await;
    let after = crate::TurnObservation::now()
        .expect("clock")
        .unix_milliseconds();
    assert_eq!(record.spec.agent_id, parent);
    assert_eq!(record.spec.name, format!("Follow-up: {NOTE}"));
    assert_eq!(record.spec.prompt, NOTE);
    assert!(record.spec.enabled);
    assert!(matches!(
        record.spec.schedule,
        AutomationSchedule::Once { .. }
    ));
    let day = 1440 * 60_000;
    assert!(
        (before + day..=after + day).contains(&record.next_due_ms),
        "due a day from the call: {}",
        record.next_due_ms
    );
    assert_eq!(
        h.list_automations(parent, parent, None)
            .await
            .expect("listed")
            .len(),
        1,
        "the agent sees its follow-up with automation_manage"
    );
    let scheduler = h.automation_scheduler().expect("scheduler");
    let due = scheduler
        .next_run(record.next_due_ms, false)
        .await
        .expect("admit")
        .expect("due");
    assert_eq!(due.run.automation_id, record.id);
    assert_eq!(
        due.origin_session_id.map(|session| session.to_string()),
        Some(chat_id.to_string()),
        "it comes back to the conversation it was set in"
    );
    assert!(due.run.submission.ends_with(NOTE));
    scheduler
        .finish_run(
            due.run.id,
            landed("The table is booked.", RunLanding::Conversation),
            record.next_due_ms + 1,
        )
        .await
        .expect("record");
    assert!(
        matches!(
            h.automation(parent, record.id).await,
            Err(LocalHostError::Automation(AutomationError::NotFound))
        ),
        "a fulfilled follow-up is deleted"
    );
    assert!(
        h.list_automations(parent, parent, None)
            .await
            .expect("listed")
            .is_empty()
    );
    assert!(
        h.automation_results(parent, parent, None)
            .await
            .expect("results")
            .is_empty(),
        "and purged with its run"
    );
}

/// A follow-up whose run failed, or found its conversation gone, stays
/// disarmed with its result, as any one-time automation does.
#[tokio::test]
async fn a_follow_up_that_failed_or_found_no_conversation_stays() {
    let (_d, h, parent, _) = fixture().await;
    let (chat, _) = conversation(&h, parent).await;
    let failing = set_by_model(&h, parent, &chat, 60).await;
    let homeless = set_by_model(&h, parent, &chat, 120).await;
    let scheduler = h.automation_scheduler().expect("scheduler");
    for (record, outcome) in [
        (
            &failing,
            RunOutcome::Failed {
                reason: "the model was unavailable".to_owned(),
                failed_tool_calls: 0,
            },
        ),
        (&homeless, landed("Asked.", RunLanding::OwnTask)),
    ] {
        let due = scheduler
            .next_run(record.next_due_ms, false)
            .await
            .expect("admit")
            .expect("due");
        assert_eq!(due.run.automation_id, record.id);
        scheduler
            .finish_run(due.run.id, outcome, record.next_due_ms + 1)
            .await
            .expect("record");
        let kept = h.automation(parent, record.id).await.expect("kept");
        assert!(!kept.spec.enabled, "disarmed");
    }
    assert_eq!(
        h.automation_results(parent, parent, None)
            .await
            .expect("results")
            .len(),
        2
    );
}

/// A follow-up its agent armed again while it ran is kept by its run's
/// success, armed.
#[tokio::test]
async fn a_follow_up_armed_again_is_kept() {
    let (_d, h, parent, _) = fixture().await;
    let (chat, _) = conversation(&h, parent).await;
    let record = set_by_model(&h, parent, &chat, 60).await;
    let scheduler = h.automation_scheduler().expect("scheduler");
    let due = scheduler
        .next_run(record.next_due_ms, false)
        .await
        .expect("admit")
        .expect("due");
    let disarmed = h.automation(parent, record.id).await.expect("disarmed");
    let mut spec = disarmed.spec.clone();
    spec.enabled = true;
    spec.schedule = AutomationSchedule::Once {
        at: "2100-01-01T09:00:00Z".to_owned(),
    };
    h.manage_automation(
        parent,
        Uuid::new_v4(),
        AutomationMutation::Update {
            id: record.id,
            expected_revision: disarmed.revision,
            spec,
        },
        record.next_due_ms,
        CancellationToken::new(),
    )
    .await
    .expect("armed again");
    scheduler
        .finish_run(
            due.run.id,
            landed("Asked.", RunLanding::Conversation),
            record.next_due_ms + 1,
        )
        .await
        .expect("record");
    let kept = h.automation(parent, record.id).await.expect("kept");
    assert!(kept.spec.enabled, "still armed");
}

/// An agent may have [`follow_up::WAITING_MAX`] follow-ups waiting; one that
/// has started no longer waits. A replayed call returns the follow-up it set.
#[tokio::test]
async fn the_follow_ups_waiting_are_bounded_and_a_replay_sets_none() {
    let (_d, h, parent, _) = fixture().await;
    let (_chat, session) = conversation(&h, parent).await;
    let command = CommandId::new();
    let first = call(&h, parent, session, command, "call-0").await;
    assert!(!first.is_error, "{}", text(&first));
    let replayed = call(&h, parent, session, command, "call-0").await;
    assert_eq!(
        text(&replayed),
        text(&first),
        "a replay returns what it set"
    );
    let reused = call_after(&h, parent, session, command, "call-0", 90).await;
    assert!(reused.is_error);
    assert_eq!(
        text(&reused),
        "This call's id already set a different follow-up, so nothing new was set. Call follow_up again as a new call."
    );
    for index in 1..follow_up::WAITING_MAX {
        let set = call(&h, parent, session, command, &format!("call-{index}")).await;
        assert!(!set.is_error, "{}", text(&set));
    }
    let full = call(&h, parent, session, command, "call-full").await;
    assert!(full.is_error);
    assert_eq!(
        text(&full),
        "You already have 20 follow-ups waiting; the limit is 20. Let one run, or remove one with automation_manage (action delete), then try again."
    );
    let listed = h
        .list_automations(parent, parent, None)
        .await
        .expect("listed");
    assert_eq!(
        i64::try_from(listed.len()).expect("count"),
        follow_up::WAITING_MAX,
        "the replay and the refusal set nothing"
    );
    let scheduler = h.automation_scheduler().expect("scheduler");
    let due_ms = listed
        .iter()
        .map(|record| record.next_due_ms)
        .max()
        .expect("due");
    scheduler
        .next_run(due_ms, false)
        .await
        .expect("admit")
        .expect("one starts");
    let room = call(&h, parent, session, command, "call-room").await;
    assert!(!room.is_error, "{}", text(&room));
}

/// A schema 50 catalog gains the mark of follow-ups when the Host opens it.
#[tokio::test]
async fn schema_51_lets_a_schema_50_host_set_follow_ups() {
    let (d, h, parent, _) = fixture().await;
    let (chat, session) = conversation(&h, parent).await;
    drop(chat);
    let connection = rusqlite::Connection::open(&h.config.database).expect("catalog");
    follow_up::restore_schema_50(&connection);
    connection
        .execute_batch(
            "UPDATE host_metadata SET schema_version = 50 WHERE singleton = 1;
             PRAGMA user_version = 50;",
        )
        .expect("store schema 50");
    drop(connection);
    drop(h);
    let h = host(d.path());
    let set = call(&h, parent, session, CommandId::new(), "call").await;
    assert!(!set.is_error, "{}", text(&set));
}

/// Only a follow-up goes when its run succeeds: a one-time automation the
/// agent made from the same conversation stays, disarmed, with its result.
#[tokio::test]
async fn a_one_time_automation_that_is_no_follow_up_is_kept_after_its_run() {
    let (_d, h, parent, _) = fixture().await;
    let (chat, _) = conversation(&h, parent).await;
    chat.execute_turn(
        Uuid::new_v4(),
        vec![ContentBlock::text(format!(
            "create once {parent} 2100-01-01T14:00:00+05:30"
        ))],
        Arc::new(Quiet),
    )
    .await
    .expect("the model schedules it");
    let record = h
        .list_automations(parent, parent, None)
        .await
        .expect("listed")
        .remove(0);
    let scheduler = h.automation_scheduler().expect("scheduler");
    let due = scheduler
        .next_run(record.next_due_ms, false)
        .await
        .expect("admit")
        .expect("due");
    assert!(
        due.origin_session_id.is_some(),
        "it returns to its conversation"
    );
    scheduler
        .finish_run(
            due.run.id,
            landed("Digest saved: digest.md", RunLanding::Conversation),
            record.next_due_ms + 1,
        )
        .await
        .expect("record");
    let kept = h.automation(parent, record.id).await.expect("kept");
    assert!(!kept.spec.enabled, "disarmed");
    assert_eq!(
        h.automation_results(parent, parent, None)
            .await
            .expect("results")
            .len(),
        1
    );
}

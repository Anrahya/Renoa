use super::*;
use crate::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, AgentSession,
    HostCatalogError, LocalTurnOutcome, ModelProvider, host::HostInitialization,
};
use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock};
use renoa_kernel::AgentId;
use serde_json::{Value, json};
use std::{fs, path::Path, sync::Arc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct Quiet;

/// An executed run's successful outcome with no failed tool calls.
fn succeeded(answer: &str) -> RunOutcome {
    RunOutcome::Succeeded {
        answer: answer.to_owned(),
        failed_tool_calls: 0,
    }
}

#[test]
fn automation_model_schemas_avoid_unaccepted_unions_and_keep_action_guidance() {
    fn assert_no_one_of(value: &Value) {
        match value {
            Value::Object(fields) => {
                assert!(
                    !fields.contains_key("oneOf"),
                    "provider rejects oneOf: {value}"
                );
                for child in fields.values() {
                    assert_no_one_of(child);
                }
            }
            Value::Array(values) => {
                for child in values {
                    assert_no_one_of(child);
                }
            }
            _ => {}
        }
    }

    let manage = tool::input_schema();
    let results = result_tool::input_schema();
    assert_no_one_of(&manage);
    assert_no_one_of(&results);
    assert_eq!(
        manage["properties"]["action"]["enum"],
        json!(["list", "get", "create", "update", "run_now", "delete"])
    );
    assert_eq!(
        manage["properties"]["spec"]["properties"]["schedule"]["properties"]["kind"]["enum"],
        json!(["once", "cron"])
    );
    assert_eq!(
        results["properties"]["action"]["enum"],
        json!(["list", "read"])
    );
}

impl AgentEventSink for Quiet {
    fn emit(&self, _: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}
fn try_host(root: &Path) -> Result<LocalHost, LocalHostError> {
    LocalHost::assemble(HostInitialization {
        data_directory: root.join("data"),
        bridge: root.join("model.mjs"),
        providers: vec![ModelProvider::Xai],
        initial_provider: ModelProvider::Xai,
        initial_model: "fixture".to_owned(),
        initial_reasoning: None,
        credential_store: root.join("auth.sqlite"),
        mcp_adapter: None,
        mcp_registry_adapter: None,
        shared_plugin_registry: None,
        global_skill_source: None,
        oauth_relay: None,
        code_mode: None,
    })
}
fn host(root: &Path) -> LocalHost {
    try_host(root).expect("host")
}
async fn provisioned(h: &LocalHost) -> (AgentId, AgentId) {
    let creator = AgentCreator::System {
        component: "automation-fixture".to_owned(),
    };
    let parent = h
        .create_agent(
            creator.clone(),
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::from_preset(
                Uuid::new_v4(),
                AgentPresetId::new(crate::presets::GENERAL_PRESET_ID).expect("preset"),
                "Operator",
            )
            .with_instructions("Manage automations."),
            CancellationToken::new(),
        )
        .await
        .expect("operator")
        .id;
    let child = h
        .create_agent(
            creator,
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::from_preset(
                Uuid::new_v4(),
                AgentPresetId::new(crate::presets::GENERAL_PRESET_ID).expect("preset"),
                "Digest",
            )
            .with_instructions("Write a digest.")
            .with_tools(["write_file".to_owned()]),
            CancellationToken::new(),
        )
        .await
        .expect("specialist")
        .id;
    (parent, child)
}
async fn fixture() -> (tempfile::TempDir, LocalHost, AgentId, AgentId) {
    let d = tempfile::tempdir().expect("directory");
    fs::write(
        d.path().join("model.mjs"),
        concat!(
            include_str!("../../../tests/support/plugin_driver.mjs"),
            include_str!("test_model.mjs")
        ),
    )
    .expect("model");
    fs::write(d.path().join("auth.sqlite"), "").expect("auth boundary");
    let h = host(d.path());
    let (parent, child) = provisioned(&h).await;
    (d, h, parent, child)
}
async fn outsider(h: &LocalHost) -> AgentId {
    let agent = h
        .create_agent(
            AgentCreator::System {
                component: "automation-fixture".to_owned(),
            },
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::from_preset(
                Uuid::new_v4(),
                AgentPresetId::new(crate::presets::GENERAL_PRESET_ID).expect("preset"),
                "Outsider",
            )
            .with_instructions("Do unrelated work."),
            CancellationToken::new(),
        )
        .await
        .expect("outsider")
        .id;
    crate::plugins::host::state::change(
        &h.config.database,
        agent,
        crate::plugins::host::HostPluginId::Agents,
        false,
        "disable-agent-management",
    )
    .expect("disable outsider agent management");
    agent
}
fn cron(expression: &str, timezone: &str) -> AutomationSchedule {
    AutomationSchedule::Cron {
        expression: expression.to_owned(),
        timezone: timezone.to_owned(),
    }
}
fn spec(agent_id: AgentId) -> AutomationSpec {
    AutomationSpec {
        agent_id,
        name: "Digest".to_owned(),
        prompt: "scheduled digest".to_owned(),
        schedule: cron("0 */12 * * *", "UTC"),
        enabled: true,
    }
}
async fn turn(session: &AgentSession, prompt: String, label: &str) -> String {
    match session
        .execute_turn(
            Uuid::new_v4(),
            vec![ContentBlock::text(prompt)],
            Arc::new(Quiet),
        )
        .await
        .unwrap_or_else(|error| panic!("{label} failed: {error}"))
    {
        LocalTurnOutcome::Completed { output, .. } => output,
        other => panic!("{label} did not complete: {other:?}"),
    }
}

#[tokio::test]
async fn edits_replay_exactly_conflict_with_stale_revisions_and_do_not_mutate_admitted_runs() {
    let (_d, h, parent, child) = fixture().await;
    let op = Uuid::new_v4();
    let create = AutomationMutation::Create { spec: spec(child) };
    let first = h
        .manage_automation(parent, op, create.clone(), 1000, CancellationToken::new())
        .await
        .expect("create");
    assert_eq!(
        first,
        h.manage_automation(parent, op, create, 5000, CancellationToken::new())
            .await
            .expect("replay ignores new clock")
    );
    let run = runs::next(&h.config.database, first.next_due_ms + 60_000)
        .expect("admit on time")
        .expect("run");
    assert_eq!(run.due_ms, first.next_due_ms);
    let mut changed = first.spec.clone();
    changed.prompt = "changed task".to_owned();
    changed.schedule = cron("0 0 * * *", "UTC");
    let update_op = Uuid::new_v4();
    let update = AutomationMutation::Update {
        id: first.id,
        expected_revision: 1,
        spec: changed,
    };
    let second = h
        .manage_automation(
            child,
            update_op,
            update.clone(),
            run.due_ms,
            CancellationToken::new(),
        )
        .await
        .expect("self edit");
    assert_eq!(second.revision, 2);
    assert_eq!(
        second,
        h.manage_automation(
            child,
            update_op,
            update.clone(),
            run.due_ms + 1,
            CancellationToken::new()
        )
        .await
        .expect("exact replay")
    );
    assert!(
        h.manage_automation(
            child,
            Uuid::new_v4(),
            update,
            run.due_ms,
            CancellationToken::new()
        )
        .await
        .is_err()
    );
    let resumed = runs::next(&h.config.database, run.due_ms + 200_000_000)
        .expect("resume")
        .expect("same occurrence");
    assert_eq!(run, resumed);
    assert!(run.submission.ends_with("\n\nscheduled digest"));
    assert!(
        h.manage_automation(
            child,
            Uuid::new_v4(),
            AutomationMutation::RunNow { id: first.id },
            run.due_ms,
            CancellationToken::new()
        )
        .await
        .is_err()
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(
        h.manage_automation(
            parent,
            Uuid::new_v4(),
            AutomationMutation::Create { spec: spec(child) },
            0,
            cancelled
        )
        .await
        .is_err()
    );
    assert!(
        h.manage_automation(
            outsider(&h).await,
            Uuid::new_v4(),
            AutomationMutation::Create { spec: spec(child) },
            0,
            CancellationToken::new()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn real_model_tool_schedules_a_specialist_that_reschedules_its_own_automation() {
    let (d, h, parent, child) = fixture().await;
    let workspace = d.path().join("workspace");
    fs::create_dir(&workspace).expect("workspace");
    let session = h
        .ensure_agent_session(parent, &workspace, Uuid::new_v4())
        .await
        .expect("parent session");
    let prompt = format!("create automation {child}");
    let result = session
        .execute_turn(
            Uuid::new_v4(),
            vec![ContentBlock::text(prompt)],
            Arc::new(Quiet),
        )
        .await
        .expect("management through model");
    assert!(matches!(result, LocalTurnOutcome::Completed { .. }));
    let records = h
        .list_automations(parent, child, None)
        .await
        .expect("automations");
    assert_eq!(records.len(), 1);
    let scheduler = h.automation_scheduler().expect("own the schedule");
    let due = scheduler
        .next_run(records[0].next_due_ms)
        .await
        .expect("admit")
        .expect("due");
    assert_eq!(
        due.origin_session_id, None,
        "an automation made for another agent runs in a conversation of its own"
    );
    scheduler
        .finish_run(due.run.id, succeeded("Digest saved: digest.md"), 0)
        .await
        .expect("record the result");
    drop(scheduler);
    drop(session);
    drop(h);
    let restarted = host(d.path());
    let child_workspace = restarted.agent_workspace(child).await.expect("workspace");
    let specialist_session = restarted
        .ensure_agent_session(child, &child_workspace, Uuid::new_v4())
        .await
        .expect("interactive specialist");
    specialist_session
        .execute_turn(
            Uuid::new_v4(),
            vec![ContentBlock::text(format!("reschedule {}", records[0].id))],
            Arc::new(Quiet),
        )
        .await
        .expect("specialist changes own schedule");
    let changed = restarted
        .list_automations(parent, child, None)
        .await
        .expect("new schedule");
    assert_eq!(changed[0].revision, 2);
    assert_eq!(changed[0].spec.schedule, cron("0 0 * * *", "UTC"));
}

#[tokio::test]
async fn schema_fifteen_upgrade_preserves_agent_identity_and_the_schedule_has_one_owner() {
    let (d, h, _parent, child) = fixture().await;
    let identity = h.host_id().await.expect("identity");
    let db = crate::host::catalog::open_verified(&h.config.database).expect("database");
    db.execute_batch("DROP TABLE host_automation_deletions; DROP TABLE host_automation_mutations; DROP TABLE host_automation_runs; DROP TABLE host_automations; UPDATE host_metadata SET schema_version=15; PRAGMA user_version=15;").expect("schema fifteen");
    drop(db);
    drop(h);
    let refused = try_host(d.path());
    assert!(
        matches!(&refused, Err(LocalHostError::HostCatalog(HostCatalogError::Invalid(message))) if message.contains("reset")),
        "an earlier data root must be refused until it is reset: {:?}",
        refused.as_ref().err()
    );
    crate::reset_host_data_root(&d.path().join("data")).expect("cutover reset");
    let restored = host(d.path());
    assert_eq!(restored.host_id().await.expect("retained Host"), identity);
    assert!(
        restored
            .agent_definition(child)
            .await
            .expect("agent discarded")
            .is_none()
    );
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(restored.config.database.with_file_name(".automations.lock"))
        .expect("lease");
    lock.try_lock().expect("first owner");
    assert!(restored.automation_scheduler().is_err());
    lock.unlock()
        .expect("release simulated owner before inherited fork descriptors close");
    drop(lock);
    let scheduler = restored
        .automation_scheduler()
        .expect("new owner after release");
    assert!(
        restored.automation_scheduler().is_err(),
        "one process owns the schedule at a time"
    );
    drop(scheduler);
}

#[tokio::test]
async fn an_automation_an_agent_creates_for_itself_returns_to_the_creating_session() {
    let (d, h, parent, _child) = fixture().await;
    let workspace = d.path().join("workspace");
    fs::create_dir(&workspace).expect("workspace");
    let session_id = Uuid::new_v4();
    let session = h
        .ensure_agent_session(parent, &workspace, session_id)
        .await
        .expect("parent session");
    session
        .execute_turn(
            Uuid::new_v4(),
            vec![ContentBlock::text(format!("create automation {parent}"))],
            Arc::new(Quiet),
        )
        .await
        .expect("management through model");
    let record = h
        .list_automations(parent, parent, None)
        .await
        .expect("automations")
        .remove(0);
    let scheduler = h.automation_scheduler().expect("own the schedule");
    let due = scheduler
        .next_run(record.next_due_ms)
        .await
        .expect("admit")
        .expect("due");
    assert_eq!(due.run.automation_id, record.id);
    assert_eq!(due.origin_session_id, Some(session_id));
}

#[tokio::test]
async fn paused_automations_allow_one_idempotent_manual_run_and_intervals_keep_their_phase() {
    let (_d, h, parent, child) = fixture().await;
    let mut paused = spec(child);
    paused.enabled = false;
    let automation = h
        .manage_automation(
            parent,
            Uuid::new_v4(),
            AutomationMutation::Create { spec: paused },
            0,
            CancellationToken::new(),
        )
        .await
        .expect("paused automation");
    assert!(
        runs::next(&h.config.database, 100_000_000)
            .expect("paused")
            .is_none()
    );
    let op = Uuid::new_v4();
    let manual = AutomationMutation::RunNow { id: automation.id };
    h.manage_automation(
        child,
        op,
        manual.clone(),
        100_000_000,
        CancellationToken::new(),
    )
    .await
    .expect("manual run");
    h.manage_automation(child, op, manual, 200_000_000, CancellationToken::new())
        .await
        .expect("manual replay");
    let admitted = runs::next(&h.config.database, 200_000_000)
        .expect("queue")
        .expect("manual");
    assert_eq!(admitted.id, op);
    assert_eq!(admitted.admitted_at_ms, 100_000_000);
    runs::finish(&h.config.database, op, &succeeded("done"), 0).expect("complete");
    assert!(
        runs::next(&h.config.database, 300_000_000)
            .expect("no recurring run")
            .is_none()
    );
    assert_eq!(
        admitted.submission, "(Requested run of \"Digest\".)\n\nscheduled digest",
        "a requested run has no due time"
    );
}

#[tokio::test]
async fn automation_reads_apply_the_same_actor_rule_as_mutations() {
    let (_d, h, parent, child) = fixture().await;
    crate::plugins::host::state::change(
        &h.config.database,
        child,
        crate::plugins::host::HostPluginId::Agents,
        false,
        "own-automations-only",
    )
    .expect("restrict management");
    let own = h
        .manage_automation(
            parent,
            Uuid::new_v4(),
            AutomationMutation::Create { spec: spec(child) },
            0,
            CancellationToken::new(),
        )
        .await
        .expect("own automation");
    let foreign = h
        .manage_automation(
            parent,
            Uuid::new_v4(),
            AutomationMutation::Create { spec: spec(parent) },
            0,
            CancellationToken::new(),
        )
        .await
        .expect("foreign automation");
    let child_workspace = h.agent_workspace(child).await.expect("workspace");
    let specialist = h
        .ensure_agent_session(child, &child_workspace, Uuid::new_v4())
        .await
        .expect("specialist session");
    let parent_workspace = h.agent_workspace(parent).await.expect("workspace");
    let operator = h
        .ensure_agent_session(parent, &parent_workspace, Uuid::new_v4())
        .await
        .expect("operator session");
    let denied = turn(
        &specialist,
        format!("foreign automation list {parent}"),
        "foreign list",
    )
    .await;
    assert!(
        denied.contains("renoa.agents") && !denied.contains("Digest"),
        "a specialist must not list another agent's automations: {denied}"
    );
    let denied = turn(
        &specialist,
        format!("foreign automation get {}", foreign.id),
        "foreign get",
    )
    .await;
    assert!(
        denied.contains("renoa.agents") && !denied.contains("scheduled digest"),
        "a specialist must not read another agent's standing task: {denied}"
    );
    let own_list = turn(&specialist, "own automation list".to_owned(), "own list").await;
    assert!(
        own_list.contains("Digest"),
        "a specialist lists its own automations: {own_list}"
    );
    let own_get = turn(
        &specialist,
        format!("own automation get {}", own.id),
        "own get",
    )
    .await;
    assert!(
        own_get.contains("scheduled digest"),
        "a specialist reads its own standing task: {own_get}"
    );
    let managed_list = turn(
        &operator,
        format!("foreign automation list {child}"),
        "managed list",
    )
    .await;
    assert!(
        managed_list.contains("Digest"),
        "an agent_manage holder lists another agent's automations: {managed_list}"
    );
    let managed_get = turn(
        &operator,
        format!("foreign automation get {}", own.id),
        "managed get",
    )
    .await;
    assert!(
        managed_get.contains("scheduled digest"),
        "an agent_manage holder reads another agent's standing task: {managed_get}"
    );
}

mod once;
mod results;
mod retention;

mod control;
mod cron;
mod deletion;
mod outcomes;

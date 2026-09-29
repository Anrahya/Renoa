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
        json!(["once", "daily", "interval"])
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
fn spec(agent_id: AgentId) -> AutomationSpec {
    AutomationSpec {
        agent_id,
        name: "Digest".to_owned(),
        prompt: "scheduled digest".to_owned(),
        schedule: AutomationSchedule::Interval { hours: 12 },
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
    let run = store::next(&h.config.database, first.next_due_ms + 100_000_000)
        .expect("admit catchup")
        .expect("run");
    assert_eq!(run.due_ms, first.next_due_ms);
    let mut changed = first.spec.clone();
    changed.prompt = "changed task".to_owned();
    changed.schedule = AutomationSchedule::Interval { hours: 24 };
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
    let resumed = store::next(&h.config.database, run.due_ms + 200_000_000)
        .expect("resume")
        .expect("same occurrence");
    assert_eq!(run, resumed);
    assert_eq!(run.prompt, "scheduled digest");
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

#[test]
fn daily_schedules_respect_local_time_and_daylight_saving() {
    let ms = |value: &str| {
        value
            .parse::<jiff::Timestamp>()
            .expect("timestamp")
            .as_millisecond()
    };
    let daily = AutomationSchedule::Daily {
        hour: 14,
        minute: 0,
        timezone: "Asia/Kolkata".to_owned(),
    };
    assert_eq!(
        daily
            .next_after(ms("2026-09-07T08:29:00Z"))
            .expect("same day"),
        ms("2026-09-07T08:30:00Z")
    );
    assert_eq!(
        daily
            .next_after(ms("2026-09-07T08:30:00Z"))
            .expect("next day"),
        ms("2026-09-08T08:30:00Z")
    );
    let spring = AutomationSchedule::Daily {
        hour: 2,
        minute: 30,
        timezone: "America/New_York".to_owned(),
    };
    assert_eq!(
        spring
            .next_after(ms("2026-03-08T05:00:00Z"))
            .expect("spring gap"),
        ms("2026-03-08T07:30:00Z")
    );
    let fall = AutomationSchedule::Daily {
        hour: 1,
        minute: 30,
        timezone: "America/New_York".to_owned(),
    };
    assert_eq!(
        fall.next_after(ms("2026-11-01T05:30:00Z"))
            .expect("no duplicate fall occurrence"),
        ms("2026-11-02T06:30:00Z")
    );
    assert!(
        AutomationSchedule::Interval { hours: 0 }
            .next_after(0)
            .is_err()
    );
}

#[tokio::test]
async fn real_model_tool_schedules_a_specialist_and_restarted_host_replays_execution_without_rewriting_artifact()
 {
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
    let run = store::next(&h.config.database, records[0].next_due_ms)
        .expect("admit")
        .expect("due");
    // Execute the real kernel, then simulate losing only the Host output receipt.
    h.execute_automation_run(run.clone()).await.expect("run");
    let child_workspace = h.agent_workspace(child).await.expect("workspace");
    assert_eq!(
        fs::read_to_string(child_workspace.join("digest.md")).expect("artifact"),
        "# Digest\nSaved by the specialist."
    );
    let db = crate::host::catalog::open_verified(&h.config.database).expect("catalog");
    db.execute(
        "UPDATE host_automation_runs SET output=NULL WHERE id=?1",
        [run.id.to_string()],
    )
    .expect("lost receipt");
    drop(db);
    drop(session);
    drop(h);
    fs::write(
        child_workspace.join("digest.md"),
        "preserve after completed execution",
    )
    .expect("marker");
    let restarted = host(d.path());
    let pending = store::next(&restarted.config.database, run.due_ms + 1)
        .expect("recover")
        .expect("pending");
    assert_eq!(pending.id, run.id);
    restarted
        .execute_automation_run(pending)
        .await
        .expect("kernel replay");
    assert_eq!(
        fs::read_to_string(child_workspace.join("digest.md")).expect("preserved"),
        "preserve after completed execution"
    );
    let outputs = restarted
        .completed_automation_runs(0)
        .await
        .expect("outputs");
    assert_eq!(outputs.len(), 1);
    assert!(
        outputs[0]
            .output
            .as_deref()
            .expect("output")
            .contains("digest.md")
    );
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
    assert_eq!(
        changed[0].spec.schedule,
        AutomationSchedule::Interval { hours: 24 }
    );
}

#[tokio::test]
async fn schema_fifteen_upgrade_preserves_agent_identity_and_runner_has_exclusive_ownership() {
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
    assert!(
        restored
            .run_automations(CancellationToken::new())
            .await
            .is_err()
    );
    lock.unlock()
        .expect("release simulated owner before inherited fork descriptors close");
    drop(lock);
    let stop = CancellationToken::new();
    stop.cancel();
    restored
        .run_automations(stop)
        .await
        .expect("new owner after release");
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
        store::next(&h.config.database, 100_000_000)
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
    let admitted = store::next(&h.config.database, 200_000_000)
        .expect("queue")
        .expect("manual");
    assert_eq!(admitted.id, op);
    assert_eq!(admitted.admitted_at_ms, 100_000_000);
    store::finish(&h.config.database, op, "done").expect("complete");
    assert!(
        store::next(&h.config.database, 300_000_000)
            .expect("no recurring run")
            .is_none()
    );
    let interval = AutomationSchedule::Interval { hours: 12 };
    assert_eq!(
        interval
            .advance_past(43_200_000, 90_000_000)
            .expect("retain phase"),
        129_600_000
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

mod control;
mod deletion;

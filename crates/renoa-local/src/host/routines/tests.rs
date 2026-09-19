use super::*;
use crate::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, HostCatalogError,
    LocalTurnOutcome, ModelProvider, host::HostInitialization,
};
use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock};
use renoa_kernel::AgentId;
use std::{fs, path::Path, sync::Arc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

struct Quiet;
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
    })
}
fn host(root: &Path) -> LocalHost {
    try_host(root).expect("host")
}
async fn provisioned(h: &LocalHost) -> (AgentId, AgentId) {
    let creator = AgentCreator::System {
        component: "routine-fixture".to_owned(),
    };
    let parent = h
        .create_agent(
            creator.clone(),
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::new(
                Uuid::new_v4(),
                AgentPresetId::new(crate::presets::SPECIALIST_PRESET_ID).expect("preset"),
                "Operator",
            )
            .with_instructions("Manage routines.")
            .with_tools([crate::capabilities::AGENT_MANAGE.to_owned()]),
            CancellationToken::new(),
        )
        .await
        .expect("operator")
        .id;
    let child = h
        .create_agent(
            creator,
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::new(
                Uuid::new_v4(),
                AgentPresetId::new(crate::presets::SPECIALIST_PRESET_ID).expect("preset"),
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
    fs::write(d.path().join("model.mjs"), include_str!("test_model.mjs")).expect("model");
    fs::write(d.path().join("auth.sqlite"), "").expect("auth boundary");
    let h = host(d.path());
    let (parent, child) = provisioned(&h).await;
    (d, h, parent, child)
}
async fn outsider(h: &LocalHost) -> AgentId {
    h.create_agent(
        AgentCreator::System {
            component: "routine-fixture".to_owned(),
        },
        AgentCreationOrigin::Provisioning,
        AgentCreateRequest::new(
            Uuid::new_v4(),
            AgentPresetId::new(crate::presets::SPECIALIST_PRESET_ID).expect("preset"),
            "Outsider",
        )
        .with_instructions("Do unrelated work."),
        CancellationToken::new(),
    )
    .await
    .expect("outsider")
    .id
}
fn spec(agent_id: AgentId) -> RoutineSpec {
    RoutineSpec {
        agent_id,
        name: "Digest".to_owned(),
        prompt: "scheduled digest".to_owned(),
        schedule: RoutineSchedule::Interval { hours: 12 },
        enabled: true,
    }
}

#[tokio::test]
async fn edits_replay_exactly_conflict_with_stale_revisions_and_do_not_mutate_admitted_runs() {
    let (_d, h, parent, child) = fixture().await;
    let op = Uuid::new_v4();
    let create = RoutineMutation::Create { spec: spec(child) };
    let first = h
        .manage_routine(parent, op, create.clone(), 1000, CancellationToken::new())
        .await
        .expect("create");
    assert_eq!(
        first,
        h.manage_routine(parent, op, create, 5000, CancellationToken::new())
            .await
            .expect("replay ignores new clock")
    );
    let run = store::next(&h.config.database, first.next_due_ms + 100_000_000)
        .expect("admit catchup")
        .expect("run");
    assert_eq!(run.due_ms, first.next_due_ms);
    let mut changed = first.spec.clone();
    changed.prompt = "changed task".to_owned();
    changed.schedule = RoutineSchedule::Interval { hours: 24 };
    let update_op = Uuid::new_v4();
    let update = RoutineMutation::Update {
        id: first.id,
        expected_revision: 1,
        spec: changed,
    };
    let second = h
        .manage_routine(
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
        h.manage_routine(
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
        h.manage_routine(
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
        h.manage_routine(
            child,
            Uuid::new_v4(),
            RoutineMutation::RunNow { id: first.id },
            run.due_ms,
            CancellationToken::new()
        )
        .await
        .is_err()
    );
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(
        h.manage_routine(
            parent,
            Uuid::new_v4(),
            RoutineMutation::Create { spec: spec(child) },
            0,
            cancelled
        )
        .await
        .is_err()
    );
    assert!(
        h.manage_routine(
            outsider(&h).await,
            Uuid::new_v4(),
            RoutineMutation::Create { spec: spec(child) },
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
    let daily = RoutineSchedule::Daily {
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
    let spring = RoutineSchedule::Daily {
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
    let fall = RoutineSchedule::Daily {
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
        RoutineSchedule::Interval { hours: 0 }
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
    let prompt = format!("create routine {child}");
    let result = session
        .execute_turn(
            Uuid::new_v4(),
            vec![ContentBlock::text(prompt)],
            Arc::new(Quiet),
        )
        .await
        .expect("management through model");
    assert!(matches!(result, LocalTurnOutcome::Completed { .. }));
    let records = h.list_routines(child, None).await.expect("routines");
    assert_eq!(records.len(), 1);
    let run = store::next(&h.config.database, records[0].next_due_ms)
        .expect("admit")
        .expect("due");
    // Execute the real kernel, then simulate losing only the Host output receipt.
    h.execute_routine_run(run.clone()).await.expect("run");
    let child_workspace = h.agent_workspace(child).await.expect("workspace");
    assert_eq!(
        fs::read_to_string(child_workspace.join("digest.md")).expect("artifact"),
        "# Digest\nSaved by the specialist."
    );
    let db = crate::host::catalog::open_verified(&h.config.database).expect("catalog");
    db.execute(
        "UPDATE host_routine_runs SET output=NULL WHERE id=?1",
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
        .execute_routine_run(pending)
        .await
        .expect("kernel replay");
    assert_eq!(
        fs::read_to_string(child_workspace.join("digest.md")).expect("preserved"),
        "preserve after completed execution"
    );
    let outputs = restarted.completed_routine_runs(0).await.expect("outputs");
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
        .list_routines(child, None)
        .await
        .expect("new schedule");
    assert_eq!(changed[0].revision, 2);
    assert_eq!(
        changed[0].spec.schedule,
        RoutineSchedule::Interval { hours: 24 }
    );
}

#[tokio::test]
async fn schema_fifteen_upgrade_preserves_agent_identity_and_runner_has_exclusive_ownership() {
    let (d, h, _parent, child) = fixture().await;
    let identity = h.host_id().await.expect("identity");
    let db = crate::host::catalog::open_verified(&h.config.database).expect("database");
    db.execute_batch("DROP TABLE host_routine_deletions; DROP TABLE host_routine_mutations; DROP TABLE host_routine_runs; DROP TABLE host_routines; UPDATE host_metadata SET schema_version=15; PRAGMA user_version=15;").expect("schema fifteen");
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
        .open(restored.config.database.with_file_name(".routines.lock"))
        .expect("lease");
    lock.try_lock().expect("first owner");
    assert!(
        restored
            .run_routines(CancellationToken::new())
            .await
            .is_err()
    );
    lock.unlock()
        .expect("release simulated owner before inherited fork descriptors close");
    drop(lock);
    let stop = CancellationToken::new();
    stop.cancel();
    restored
        .run_routines(stop)
        .await
        .expect("new owner after release");
}

#[tokio::test]
async fn paused_routines_allow_one_idempotent_manual_run_and_intervals_keep_their_phase() {
    let (_d, h, parent, child) = fixture().await;
    let mut paused = spec(child);
    paused.enabled = false;
    let routine = h
        .manage_routine(
            parent,
            Uuid::new_v4(),
            RoutineMutation::Create { spec: paused },
            0,
            CancellationToken::new(),
        )
        .await
        .expect("paused routine");
    assert!(
        store::next(&h.config.database, 100_000_000)
            .expect("paused")
            .is_none()
    );
    let op = Uuid::new_v4();
    let manual = RoutineMutation::RunNow { id: routine.id };
    h.manage_routine(
        child,
        op,
        manual.clone(),
        100_000_000,
        CancellationToken::new(),
    )
    .await
    .expect("manual run");
    h.manage_routine(child, op, manual, 200_000_000, CancellationToken::new())
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
    let interval = RoutineSchedule::Interval { hours: 12 };
    assert_eq!(
        interval
            .advance_past(43_200_000, 90_000_000)
            .expect("retain phase"),
        129_600_000
    );
}

mod once;
mod results;

mod control;
mod deletion;

use super::*;
use crate::{
    AgentProfile, AgentProfileId, AgentRecord, BotRecipe, BotRecord, LocalTurnOutcome,
    ModelProvider, host::HostInitialization,
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
fn host(root: &Path) -> LocalHost {
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
        profiles: vec![
            AgentProfile::new(crate::ARCEE_PROFILE_ID, "Manage routines.").expect("profile"),
        ],
    })
    .expect("host")
}
async fn fixture() -> (tempfile::TempDir, LocalHost, AgentId, AgentId) {
    let d = tempfile::tempdir().expect("directory");
    fs::write(d.path().join("model.mjs"), include_str!("test_model.mjs")).expect("model");
    fs::write(d.path().join("auth.sqlite"), "").expect("auth boundary");
    let h = host(d.path());
    let parent = AgentId::new();
    h.ensure_agent(AgentRecord {
        id: parent,
        profile: AgentProfileId::new(crate::ARCEE_PROFILE_ID).expect("profile"),
        name: "Arcee".to_owned(),
        created_by: None,
    })
    .await
    .expect("parent");
    let child = AgentId::new();
    h.ensure_bot(BotRecord {
        id: child,
        created_by: parent,
        recipe: BotRecipe {
            name: "Digest".to_owned(),
            instructions: "Write a digest.".to_owned(),
            tools: ["write_file".to_owned()].into(),
            connections: std::collections::BTreeSet::new(),
        },
    })
    .await
    .expect("bot");
    (d, h, parent, child)
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
            AgentId::new(),
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
async fn real_model_tool_schedules_a_bot_and_restarted_host_replays_execution_without_rewriting_artifact()
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
    let child_workspace = h.bot_workspace(child).await.expect("workspace");
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
    let bot_session = restarted
        .ensure_agent_session(child, &child_workspace, Uuid::new_v4())
        .await
        .expect("interactive bot");
    bot_session
        .execute_turn(
            Uuid::new_v4(),
            vec![ContentBlock::text(format!("reschedule {}", records[0].id))],
            Arc::new(Quiet),
        )
        .await
        .expect("bot changes own schedule");
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
async fn schema_fifteen_upgrade_preserves_bot_identity_and_runner_has_exclusive_ownership() {
    let (d, h, _parent, child) = fixture().await;
    let identity = h.host_id().await.expect("identity");
    let db = crate::host::catalog::open_verified(&h.config.database).expect("database");
    db.execute_batch("DROP TABLE host_routine_mutations; DROP TABLE host_routine_runs; DROP TABLE host_routines; UPDATE host_metadata SET schema_version=15; PRAGMA user_version=15;").expect("schema fifteen");
    drop(db);
    drop(h);
    let restored = host(d.path());
    assert_eq!(restored.host_id().await.expect("retained Host"), identity);
    assert!(restored.bot(child).await.expect("bot retained").is_some());
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

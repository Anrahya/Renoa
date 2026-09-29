#[allow(
    dead_code,
    unused_imports,
    reason = "the shared node fixture exposes helpers used by the other node suites"
)]
mod support;

use std::{sync::Arc, time::Duration};

use renoa_control::{TaskEvent, TaskEventKind, TaskId};
use renoa_kernel::AgentId;
use renoa_local::{
    AutomationMutation, AutomationRecord, AutomationSchedule, AutomationSpec, LocalHost,
    TurnObservation,
};
use renoa_node::RenoaNode;
use renoa_protocol::{CommandId, ExecutionEventKind, ExecutionTerminal, SurfaceRef};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use support::{
    HostFixture, TestSystem, attach, attach_after, collect_through_terminal,
    submit_when_node_is_online, wait_for_path,
};

#[tokio::test]
async fn an_automation_created_in_a_conversation_answers_in_that_conversation() {
    timeout(Duration::from_secs(20), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let host = fixture.host();
        let shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            Arc::clone(&host),
        )
        .expect("open execution node")
        .with_automations(system.enroll_surface_as("automations").await)
        .expect("own the automation schedule");
        let node_task = tokio::spawn(node.run(shutdown.clone()));

        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        submit_when_node_is_online(
            &mut surface,
            system.task_id,
            CommandId::new(),
            &format!("Schedule for {}: Write the digest.", fixture.agent_id),
        )
        .await;
        collect_through_terminal(&mut surface).await;
        let automation = host
            .list_automations(fixture.agent_id, fixture.agent_id, None)
            .await
            .expect("list automations")
            .remove(0);
        let run = run_now(&host, fixture.agent_id, &automation).await;

        let events = collect_through_terminal(&mut surface).await;
        assert_submitted_by_automations(&events, run, "Write the digest.", &system);
        assert!(answered(&events, "Digest written."));
        assert!(completed(&events));
        assert_eq!(
            result_of(&host, fixture.agent_id, run).await,
            "Digest written."
        );
        assert_eq!(
            fixture.operation_count(),
            2,
            "the run continued the conversation's Host session"
        );

        shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    })
    .await
    .expect("conversation automation test timed out");
}

#[tokio::test]
async fn an_automation_made_outside_a_conversation_runs_in_its_own_task() {
    timeout(Duration::from_secs(20), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let host = fixture.host();
        let automation = create(&host, fixture.agent_id, "Write the digest.").await;
        // Admitted before any scheduler runs, like a run due during downtime.
        let run = run_now(&host, fixture.agent_id, &automation).await;

        let shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            Arc::clone(&host),
        )
        .expect("open execution node")
        .with_automations(system.enroll_surface_as("automations").await)
        .expect("own the automation schedule");
        let node_task = tokio::spawn(node.run(shutdown.clone()));
        assert_eq!(
            result_of(&host, fixture.agent_id, run).await,
            "Digest written."
        );

        let own_task = TaskId::from_uuid(automation.id);
        let mut surface = system.connect_surface().await;
        attach_after(&mut surface, own_task, None).await;
        let events = collect_through_terminal(&mut surface).await;
        assert_submitted_by_automations(&events, run, "Write the digest.", &system);
        assert!(answered(&events, "Digest written."));

        shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    })
    .await
    .expect("own-task automation test timed out");
}

#[tokio::test]
async fn a_node_restart_during_a_run_neither_drops_nor_repeats_it() {
    timeout(Duration::from_secs(30), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let host = fixture.host();
        let automation = create(&host, fixture.agent_id, "Crash model.").await;
        let run = run_now(&host, fixture.agent_id, &automation).await;
        let node_credentials = system.enroll_node().await;
        let automation_credentials = system.enroll_surface_as("automations").await;

        let first_shutdown = CancellationToken::new();
        let first = RenoaNode::open(
            system.url.clone(),
            node_credentials.clone(),
            Arc::clone(&host),
        )
        .expect("open first node")
        .with_automations(automation_credentials.clone())
        .expect("own the automation schedule");
        let first_task = tokio::spawn(first.run(first_shutdown.clone()));
        wait_for_path(&fixture.started()).await;
        first_shutdown.cancel();
        first_task
            .await
            .expect("first node task")
            .expect("first node stops cleanly");

        let shutdown = CancellationToken::new();
        let restarted = RenoaNode::open(system.url.clone(), node_credentials, Arc::clone(&host))
            .expect("open restarted node")
            .with_automations(automation_credentials)
            .expect("the stopped node released the schedule");
        let node_task = tokio::spawn(restarted.run(shutdown.clone()));
        assert_eq!(
            result_of(&host, fixture.agent_id, run).await,
            "Recovered the same Host turn."
        );
        assert_eq!(fixture.attempts(), "2");

        let own_task = TaskId::from_uuid(automation.id);
        assert_eq!(
            fixture.operation_count_for(own_task),
            1,
            "the restart re-drove the same Host operation"
        );
        let mut surface = system.connect_surface().await;
        attach_after(&mut surface, own_task, None).await;
        let events = collect_through_terminal(&mut surface).await;
        let submitted = events
            .iter()
            .filter(|event| matches!(event.kind, TaskEventKind::CommandSubmitted { .. }))
            .count();
        assert_eq!(submitted, 1, "the resubmitted run is one command");
        assert_eq!(
            host.completed_automation_runs(0)
                .await
                .expect("completed runs")
                .len(),
            1
        );

        shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    })
    .await
    .expect("automation restart test timed out");
}

#[tokio::test]
async fn one_node_owns_a_host_schedule_at_a_time() {
    timeout(Duration::from_secs(10), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let credentials = system.enroll_surface_as("automations").await;
        let owner = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            fixture.host(),
        )
        .expect("open owning node")
        .with_automations(credentials.clone())
        .expect("own the automation schedule");
        let second = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            fixture.host(),
        )
        .expect("open second node")
        .with_automations(credentials);
        assert!(second.is_err(), "a second scheduler is refused");
        drop(owner);
        system.stop().await;
    })
    .await
    .expect("schedule ownership test timed out");
}

async fn create(host: &LocalHost, agent: AgentId, prompt: &str) -> AutomationRecord {
    host.manage_automation(
        agent,
        Uuid::new_v4(),
        AutomationMutation::Create {
            spec: AutomationSpec {
                agent_id: agent,
                name: "Digest".to_owned(),
                prompt: prompt.to_owned(),
                schedule: AutomationSchedule::Interval { hours: 24 },
                enabled: false,
            },
        },
        now_ms(),
        CancellationToken::new(),
    )
    .await
    .expect("create automation")
}

/// Queues one run of `automation` now. The run's identity is the operation's.
async fn run_now(host: &LocalHost, agent: AgentId, automation: &AutomationRecord) -> Uuid {
    let run = Uuid::new_v4();
    host.manage_automation(
        agent,
        run,
        AutomationMutation::RunNow { id: automation.id },
        now_ms(),
        CancellationToken::new(),
    )
    .await
    .expect("queue a run");
    run
}

fn now_ms() -> i64 {
    TurnObservation::now()
        .expect("clock after the Unix epoch")
        .unix_milliseconds()
}

/// Waits for the scheduler to record the run's result on the Host.
async fn result_of(host: &LocalHost, agent: AgentId, run: Uuid) -> String {
    loop {
        if let Some(output) = host
            .automation_result(agent, run)
            .await
            .expect("read the run")
            .output
        {
            return output;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn assert_submitted_by_automations(
    events: &[TaskEvent],
    run: Uuid,
    prompt: &str,
    system: &TestSystem,
) {
    let command = events
        .iter()
        .find_map(|event| match &event.kind {
            TaskEventKind::CommandSubmitted { command }
                if command.command_id == CommandId::from_uuid(run) =>
            {
                Some(command)
            }
            _ => None,
        })
        .expect("the run is a command in the task");
    assert_eq!(command.surface, SurfaceRef::new("automations"));
    assert_eq!(command.principal_id, system.principal_id());
    assert_eq!(command.input.text(), prompt);
}

fn answered(events: &[TaskEvent], text: &str) -> bool {
    events.iter().any(|event| {
        matches!(&event.kind, TaskEventKind::ExecutionEvent { event, .. }
            if matches!(&event.kind, ExecutionEventKind::AssistantMessage { text: answer }
                if answer == text))
    })
}

fn completed(events: &[TaskEvent]) -> bool {
    matches!(
        events.last().map(|event| &event.kind),
        Some(TaskEventKind::ExecutionEvent { event, .. })
            if matches!(event.kind, ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Completed
            })
    )
}

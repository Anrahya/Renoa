mod support;

use std::time::Duration;

use renoa_control::TaskEventKind;
use renoa_local::AgentProfileId;
use renoa_node::{HostTarget, RenoaNode};
use renoa_protocol::{CommandId, ExecutionEventKind, ExecutionTerminal, SurfaceRef, TargetRef};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use support::{
    CuttableProxy, HostFixture, TestSystem, attach, attach_after, collect_through_terminal,
    collect_through_turn_started, submit_when_node_is_online, wait_for_path,
};

#[tokio::test]
async fn real_alpha_tool_turn_crosses_the_durable_rcp_bridge() {
    timeout(Duration::from_secs(10), async {
        let system = TestSystem::start().await;
        let fixture = HostFixture::install(&system);
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            system.files.path().join("node.sqlite"),
            fixture.host(),
            vec![fixture.target()],
        )
        .expect("open execution node");
        let node_task = tokio::spawn(node.run(node_shutdown.clone()));

        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        let command_id = CommandId::new();
        submit_when_node_is_online(&mut surface, system.task_id, command_id, "Read proof.").await;
        let events = collect_through_terminal(&mut surface).await;

        assert!(matches!(
            events.first().map(|event| &event.kind),
            Some(TaskEventKind::CommandSubmitted { command })
                if command.command_id == command_id
        ));
        assert_execution_event(&events, command_id, |kind| {
            matches!(kind, ExecutionEventKind::ExecutionStarted)
        });
        assert_execution_event(&events, command_id, |kind| {
            matches!(kind, ExecutionEventKind::TurnStarted)
        });
        assert_execution_event(&events, command_id, |kind| {
            matches!(kind, ExecutionEventKind::ToolStarted { call_id, name, arguments }
                if call_id == "read-proof"
                    && name == "read_file"
                    && arguments["path"] == "proof.txt")
        });
        assert_execution_event(&events, command_id, |kind| {
            matches!(kind, ExecutionEventKind::ToolFinished { call_id, output, is_error }
                if call_id == "read-proof" && output == "durable proof\n" && !is_error)
        });
        assert_execution_event(&events, command_id, |kind| {
            matches!(kind, ExecutionEventKind::AssistantMessage { text }
                if text == "The durable proof was read.")
        });
        assert!(matches!(
            events.last().map(|event| &event.kind),
            Some(TaskEventKind::ExecutionEvent { command_id: cause, event })
                if *cause == command_id
                    && matches!(event.kind, ExecutionEventKind::ExecutionTerminated {
                        terminal: ExecutionTerminal::Completed
                    })
        ));
        assert_eq!(fixture.operation_count(), 1);

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    })
    .await
    .expect("real Host bridge test timed out");
}

#[tokio::test]
async fn host_setup_failure_never_claims_that_a_turn_started() {
    timeout(Duration::from_secs(10), async {
        let system = TestSystem::start().await;
        let fixture = HostFixture::install(&system);
        let missing_profile = AgentProfileId::new("missing-profile").expect("valid profile id");
        let target = HostTarget::new(
            &system.target,
            missing_profile,
            fixture.session_id,
            &fixture.workspace,
        )
        .expect("configure missing Host profile target");
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            system.files.path().join("node.sqlite"),
            fixture.host(),
            vec![target],
        )
        .expect("open execution node");
        let node_task = tokio::spawn(node.run(node_shutdown.clone()));

        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        let command_id = CommandId::new();
        submit_when_node_is_online(&mut surface, system.task_id, command_id, "Fail setup.").await;
        let events = collect_through_terminal(&mut surface).await;

        assert!(!events.iter().any(|event| matches!(
            &event.kind,
            TaskEventKind::ExecutionEvent { event, .. }
                if matches!(event.kind, ExecutionEventKind::TurnStarted)
        )));
        assert!(matches!(
            events.last().map(|event| &event.kind),
            Some(TaskEventKind::ExecutionEvent { event, .. })
                if matches!(event.kind, ExecutionEventKind::ExecutionTerminated {
                    terminal: ExecutionTerminal::Failed { .. }
                })
        ));

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    })
    .await
    .expect("Host setup failure test timed out");
}

#[tokio::test]
async fn transport_reconnect_does_not_interrupt_the_running_host_turn() {
    timeout(Duration::from_secs(10), async {
        let system = TestSystem::start().await;
        let fixture = HostFixture::install(&system);
        let proxy = CuttableProxy::start(system.url.clone()).await;
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            proxy.url.clone(),
            system.enroll_node().await,
            system.files.path().join("node.sqlite"),
            fixture.host(),
            vec![fixture.target()],
        )
        .expect("open execution node");
        let node_task = tokio::spawn(node.run(node_shutdown.clone()));

        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        let command_id = CommandId::new();
        submit_when_node_is_online(
            &mut surface,
            system.task_id,
            command_id,
            "Hold through reconnect.",
        )
        .await;
        wait_for_path(&fixture.started()).await;
        collect_through_turn_started(&mut surface).await;

        proxy.cut().await;
        fixture.release();

        let terminal = timeout(
            Duration::from_secs(3),
            collect_through_terminal(&mut surface),
        )
        .await
        .expect("node publishes the durable suffix after reconnect");
        assert_execution_event(&terminal, command_id, |kind| {
            matches!(kind, ExecutionEventKind::AssistantMessage { text }
                if text == "Finished after reconnect.")
        });
        assert!(matches!(
            terminal.last().map(|event| &event.kind),
            Some(TaskEventKind::ExecutionEvent { command_id: cause, event })
                if *cause == command_id
                    && matches!(event.kind, ExecutionEventKind::ExecutionTerminated {
                        terminal: ExecutionTerminal::Completed
                    })
        ));
        assert_eq!(fixture.operation_count(), 1);

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node reconnects and shuts down cleanly");
        proxy.stop().await;
        system.stop().await;
    })
    .await
    .expect("transport reconnect test timed out");
}

#[tokio::test]
async fn node_restart_redrives_the_same_safe_kernel_turn() {
    timeout(Duration::from_secs(10), async {
        let system = TestSystem::start().await;
        let fixture = HostFixture::install(&system);
        let node_credentials = system.enroll_node().await;
        let node_path = system.files.path().join("node.sqlite");
        let first_shutdown = CancellationToken::new();
        let first = RenoaNode::open(
            system.url.clone(),
            node_credentials.clone(),
            &node_path,
            fixture.host(),
            vec![fixture.target()],
        )
        .expect("open first execution node");
        let first_task = tokio::spawn(first.run(first_shutdown.clone()));

        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        let command_id = CommandId::new();
        submit_when_node_is_online(&mut surface, system.task_id, command_id, "Crash model.").await;
        wait_for_path(&fixture.started()).await;
        collect_through_turn_started(&mut surface).await;

        first_shutdown.cancel();
        first_task
            .await
            .expect("first node task")
            .expect("first node stops without settling active work");

        let restarted_shutdown = CancellationToken::new();
        let restarted = RenoaNode::open(
            system.url.clone(),
            node_credentials,
            &node_path,
            fixture.host(),
            vec![fixture.target()],
        )
        .expect("reopen execution node");
        let restarted_task = tokio::spawn(restarted.run(restarted_shutdown.clone()));

        let terminal = collect_through_terminal(&mut surface).await;
        assert_execution_event(&terminal, command_id, |kind| {
            matches!(kind, ExecutionEventKind::AssistantMessage { text }
                if text == "Recovered the same Host turn.")
        });
        assert!(matches!(
            terminal.last().map(|event| &event.kind),
            Some(TaskEventKind::ExecutionEvent { command_id: cause, event })
                if *cause == command_id
                    && matches!(event.kind, ExecutionEventKind::ExecutionTerminated {
                        terminal: ExecutionTerminal::Completed
                    })
        ));
        assert_eq!(fixture.attempts(), "2");
        assert_eq!(fixture.operation_count(), 1, "kernel turn was duplicated");

        restarted_shutdown.cancel();
        restarted_task
            .await
            .expect("restarted node task")
            .expect("restarted node shuts down cleanly");
        system.stop().await;
    })
    .await
    .expect("node restart test timed out");
}

#[tokio::test]
async fn queued_turns_publish_in_host_session_order() {
    timeout(Duration::from_secs(10), async {
        let system = TestSystem::start().await;
        let fixture = HostFixture::install(&system);
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            system.files.path().join("node.sqlite"),
            fixture.host(),
            vec![fixture.target()],
        )
        .expect("open execution node");
        let node_task = tokio::spawn(node.run(node_shutdown.clone()));

        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        let first = CommandId::new();
        submit_when_node_is_online(
            &mut surface,
            system.task_id,
            first,
            "Hold through reconnect.",
        )
        .await;
        wait_for_path(&fixture.started()).await;
        collect_through_turn_started(&mut surface).await;

        let second = CommandId::new();
        submit_when_node_is_online(&mut surface, system.task_id, second, "Second.").await;
        fixture.release();

        let first_suffix = collect_through_terminal(&mut surface).await;
        assert!(matches!(
            first_suffix.last().map(|event| &event.kind),
            Some(TaskEventKind::ExecutionEvent { command_id, event })
                if *command_id == first
                    && matches!(event.kind, ExecutionEventKind::ExecutionTerminated { .. })
        ));
        assert!(first_suffix.iter().all(|event| !matches!(
            &event.kind,
            TaskEventKind::ExecutionEvent { command_id, .. } if *command_id == second
        )));

        let second_turn = collect_through_terminal(&mut surface).await;
        assert_execution_event(&second_turn, second, |kind| {
            matches!(kind, ExecutionEventKind::TurnStarted)
        });
        assert!(matches!(
            second_turn.last().map(|event| &event.kind),
            Some(TaskEventKind::ExecutionEvent { command_id, event })
                if *command_id == second
                    && matches!(event.kind, ExecutionEventKind::ExecutionTerminated {
                        terminal: ExecutionTerminal::Completed
                    })
        ));
        assert_eq!(fixture.operation_count(), 2);

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    })
    .await
    .expect("queued turn ordering test timed out");
}

#[tokio::test]
async fn independent_host_sessions_execute_in_parallel() {
    timeout(Duration::from_secs(10), async {
        let system = TestSystem::start().await;
        let fixture = HostFixture::install(&system);
        let second_target_ref = TargetRef::new("workspace:second");
        let second_task = system.create_task(second_target_ref.clone()).await;
        let second_workspace = fixture.additional_workspace();
        let second_session = Uuid::new_v4();
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            system.files.path().join("node.sqlite"),
            fixture.host(),
            vec![
                fixture.target(),
                HostFixture::target_for(&second_target_ref, second_session, &second_workspace),
            ],
        )
        .expect("open multi-session execution node");
        let node_task = tokio::spawn(node.run(node_shutdown.clone()));

        let first_credentials = system.enroll_surface_as("first").await;
        let second_credentials = system.enroll_surface_as("second").await;
        let mut first_surface = system.connect(&first_credentials).await;
        let mut second_surface = system.connect(&second_credentials).await;
        attach(&mut first_surface, system.task_id).await;
        attach(&mut second_surface, second_task).await;
        submit_when_node_is_online(
            &mut first_surface,
            system.task_id,
            CommandId::new(),
            "Parallel one.",
        )
        .await;
        submit_when_node_is_online(
            &mut second_surface,
            second_task,
            CommandId::new(),
            "Parallel two.",
        )
        .await;

        let first_started = fixture.workspace.join("model-started-one");
        let second_started = fixture.workspace.join("model-started-two");
        wait_for_path(&first_started).await;
        wait_for_path(&second_started).await;
        std::fs::write(fixture.workspace.join("model-release-one"), "release")
            .expect("release first parallel model");
        std::fs::write(fixture.workspace.join("model-release-two"), "release")
            .expect("release second parallel model");

        collect_through_terminal(&mut first_surface).await;
        collect_through_terminal(&mut second_surface).await;
        assert_eq!(fixture.operation_count(), 1);
        assert_eq!(fixture.operation_count_for(second_session), 1);

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    })
    .await
    .expect("parallel Host session test timed out");
}

#[tokio::test]
async fn independently_enrolled_surfaces_continue_one_host_session() {
    Box::pin(timeout(Duration::from_secs(10), async {
        let system = TestSystem::start().await;
        let fixture = HostFixture::install(&system);
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            system.files.path().join("node.sqlite"),
            fixture.host(),
            vec![fixture.target()],
        )
        .expect("open execution node");
        let node_task = tokio::spawn(node.run(node_shutdown.clone()));

        let linux_credentials = system.enroll_surface_as("linux").await;
        let phone_credentials = system.enroll_surface_as("phone").await;
        let mut linux = system.connect(&linux_credentials).await;
        attach(&mut linux, system.task_id).await;
        let first_command = CommandId::new();
        submit_when_node_is_online(&mut linux, system.task_id, first_command, "First.").await;
        let first_turn = collect_through_terminal(&mut linux).await;
        let first_cursor = first_turn.last().expect("first terminal event").sequence;
        assert!(matches!(
            first_turn.first().map(|event| &event.kind),
            Some(TaskEventKind::CommandSubmitted { command })
                if command.command_id == first_command
                    && command.surface == SurfaceRef::new("linux")
        ));
        drop(linux);

        let mut phone = system.connect(&phone_credentials).await;
        assert_eq!(
            attach_after(&mut phone, system.task_id, None).await,
            Some(first_cursor)
        );
        assert_eq!(collect_through_terminal(&mut phone).await, first_turn);

        let second_command = CommandId::new();
        submit_when_node_is_online(&mut phone, system.task_id, second_command, "Second.").await;
        let second_turn = collect_through_terminal(&mut phone).await;
        let second_cursor = second_turn.last().expect("second terminal event").sequence;
        assert!(matches!(
            second_turn.first().map(|event| &event.kind),
            Some(TaskEventKind::CommandSubmitted { command })
                if command.command_id == second_command
                    && command.surface == SurfaceRef::new("phone")
        ));

        let mut returned_linux = system.connect(&linux_credentials).await;
        assert_eq!(
            attach_after(&mut returned_linux, system.task_id, Some(first_cursor)).await,
            Some(second_cursor)
        );
        assert_eq!(
            collect_through_terminal(&mut returned_linux).await,
            second_turn
        );
        assert_eq!(fixture.operation_count(), 2);

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    }))
    .await
    .expect("surface handoff test timed out");
}

fn assert_execution_event(
    events: &[renoa_control::TaskEvent],
    command_id: CommandId,
    predicate: impl Fn(&ExecutionEventKind) -> bool,
) {
    assert!(
        events.iter().any(|event| matches!(
            &event.kind,
            TaskEventKind::ExecutionEvent { command_id: cause, event }
                if *cause == command_id && predicate(&event.kind)
        )),
        "missing execution event in {events:#?}"
    );
}

#[allow(
    dead_code,
    reason = "the shared node fixture exposes deletion helpers used by the automation suite"
)]
mod support;

use std::time::Duration;

use renoa_control::{TargetSummary, TaskEventKind};
use renoa_kernel::AgentId;
use renoa_node::RenoaNode;
use renoa_protocol::{CommandId, ExecutionEventKind, ExecutionTerminal, SurfaceRef};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use support::{
    CuttableProxy, HostFixture, TestSystem, agent_target, attach, attach_after,
    collect_through_terminal, collect_through_turn_started, collect_until, open_task,
    submit_when_node_is_online, wait_for_path, wait_for_targets,
};

#[tokio::test]
async fn real_alpha_tool_turn_crosses_the_durable_rcp_bridge() {
    timeout(Duration::from_secs(10), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            fixture.host(),
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
async fn tasks_opened_on_one_advertised_target_receive_separate_host_sessions() {
    timeout(Duration::from_secs(10), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            fixture.host(),
        )
        .expect("open execution node");
        let node_task = tokio::spawn(node.run(node_shutdown.clone()));
        let mut surface = system.connect_surface().await;

        assert_eq!(
            wait_for_targets(&mut surface, 1).await,
            vec![TargetSummary {
                node_id: system.node_id(),
                target: system.target.clone(),
            }]
        );
        let first = open_task(&mut surface, system.node_id(), system.target.clone()).await;
        let second = open_task(&mut surface, system.node_id(), system.target.clone()).await;
        for task in [first, second] {
            attach(&mut surface, task).await;
            submit_when_node_is_online(&mut surface, task, CommandId::new(), "Read proof.").await;
            collect_through_terminal(&mut surface).await;
        }

        assert_ne!(fixture.session_for(first), fixture.session_for(second));
        assert_eq!(fixture.operation_count_for(first), 1);
        assert_eq!(fixture.operation_count_for(second), 1);
        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    })
    .await
    .expect("runtime task opening test timed out");
}

#[tokio::test]
async fn an_agent_created_while_the_node_runs_becomes_an_advertised_target() {
    timeout(Duration::from_secs(20), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            fixture.host(),
        )
        .expect("open execution node");
        let node_task = tokio::spawn(node.run(node_shutdown.clone()));
        let mut surface = system.connect_surface().await;
        wait_for_targets(&mut surface, 1).await;

        let created = fixture.provision_agent().await;

        let targets = wait_for_targets(&mut surface, 2).await;
        assert!(
            targets
                .iter()
                .any(|summary| summary.target == agent_target(created)),
            "{targets:?}"
        );
        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    })
    .await
    .expect("agent advertisement test timed out");
}

#[tokio::test]
async fn a_turn_reads_the_user_profile_of_the_principal_that_sent_the_command() {
    timeout(Duration::from_secs(10), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let profiled = fixture
            .provision_profiled_agent(system.principal_id(), "PROFILE_OWNER\n")
            .await;
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            fixture.host(),
        )
        .expect("open execution node");
        let node_task = tokio::spawn(node.run(node_shutdown.clone()));
        let mut surface = system.connect_surface().await;
        wait_for_targets(&mut surface, 2).await;

        let task = open_task(&mut surface, system.node_id(), agent_target(profiled)).await;
        attach(&mut surface, task).await;
        let command_id = CommandId::new();
        submit_when_node_is_online(&mut surface, task, command_id, "Which profile do you see?")
            .await;
        let events = collect_through_terminal(&mut surface).await;
        assert_execution_event(&events, command_id, |kind| {
            matches!(kind, ExecutionEventKind::AssistantMessage { text } if text == "PROFILE_OWNER")
        });

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    })
    .await
    .expect("user profile test timed out");
}

#[tokio::test]
async fn agent_loss_after_startup_terminates_as_failed_without_a_turn() {
    timeout(Duration::from_secs(10), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            fixture.host(),
        )
        .expect("open execution node");
        let node_task = tokio::spawn(node.run(node_shutdown.clone()));

        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        remove_agent_definition(&fixture.data, fixture.agent_id).await;

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
    .expect("agent loss test timed out");
}

/// Removes one provisioned agent directly from the Host catalog, simulating a
/// data-root cutover that happens beneath a running node.
async fn remove_agent_definition(data: &std::path::Path, agent_id: AgentId) {
    let database = data.join("state/host.sqlite3");
    tokio::task::spawn_blocking(move || {
        let connection = rusqlite::Connection::open(&database).expect("open agent catalog");
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .expect("relax catalog foreign keys");
        let removed = connection
            .execute(
                "DELETE FROM host_agents WHERE agent_id = ?1",
                [agent_id.to_string()],
            )
            .expect("remove the agent definition");
        assert_eq!(removed, 1, "the fixture agent must exist in the catalog");
    })
    .await
    .expect("agent removal task");
}

#[tokio::test]
async fn tool_calls_and_intermediate_messages_reach_surfaces_while_the_turn_runs() {
    timeout(Duration::from_secs(10), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            fixture.host(),
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
            "Read proof, then wait.",
        )
        .await;
        // The model holds its final answer until released, so these events
        // can only arrive if the node publishes them while the turn runs.
        let mut events = collect_until(&mut surface, |event| {
            matches!(&event.kind, ExecutionEventKind::ToolFinished { call_id, .. }
                if call_id == "read-held")
        })
        .await;
        let live = execution_kinds(&events, command_id);
        let reading = live.iter().position(|kind| {
            matches!(kind, ExecutionEventKind::AssistantMessage { text }
                if text == "Reading the proof first.")
        });
        let started = live.iter().position(|kind| {
            matches!(kind, ExecutionEventKind::ToolStarted { call_id, .. } if call_id == "read-held")
        });
        assert!(
            matches!((reading, started), (Some(reading), Some(started)) if reading < started),
            "the intermediate message precedes its tool call: {live:?}"
        );

        wait_for_path(&fixture.started()).await;
        fixture.release();
        events.extend(collect_through_terminal(&mut surface).await);
        let all = execution_kinds(&events, command_id);
        let count = |matches: &dyn Fn(&ExecutionEventKind) -> bool| {
            all.iter().filter(|kind| matches(kind)).count()
        };
        assert_eq!(
            count(&|kind| matches!(kind, ExecutionEventKind::ToolStarted { .. })),
            1
        );
        assert_eq!(
            count(&|kind| matches!(kind, ExecutionEventKind::ToolFinished { .. })),
            1
        );
        assert_eq!(
            count(&|kind| matches!(kind, ExecutionEventKind::AssistantMessage { text }
                if text == "Reading the proof first.")),
            1
        );
        assert!(matches!(
            &all[all.len() - 2..],
            [
                ExecutionEventKind::AssistantMessage { text },
                ExecutionEventKind::ExecutionTerminated {
                    terminal: ExecutionTerminal::Completed
                },
            ] if text == "The held proof was read."
        ));

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    })
    .await
    .expect("live progress test timed out");
}

fn execution_kinds(
    events: &[renoa_control::TaskEvent],
    command_id: CommandId,
) -> Vec<ExecutionEventKind> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            TaskEventKind::ExecutionEvent {
                command_id: cause,
                event,
            } if *cause == command_id => Some(event.kind.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn transport_reconnect_does_not_interrupt_the_running_host_turn() {
    timeout(Duration::from_secs(10), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let proxy = CuttableProxy::start(system.url.clone()).await;
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            proxy.url.clone(),
            system.enroll_node().await,
            fixture.host(),
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
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let node_credentials = system.enroll_node().await;
        let first_shutdown = CancellationToken::new();
        let first = RenoaNode::open(system.url.clone(), node_credentials.clone(), fixture.host())
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
        let restarted = RenoaNode::open(system.url.clone(), node_credentials, fixture.host())
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
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            fixture.host(),
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
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let second_task = system.create_task(system.target.clone()).await;
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            fixture.host(),
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
        assert_eq!(fixture.operation_count_for(second_task), 1);

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
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            fixture.host(),
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

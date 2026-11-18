#[allow(
    dead_code,
    reason = "the shared node fixture exposes helpers other suites use"
)]
mod support;

use std::time::Duration;

use renoa_node::RenoaNode;
use renoa_protocol::{Author, CommandId, CommandInput, ExecutionEventKind, ExecutionTerminal};
use renoa_rcp_wire::{TaskEvent, TaskEventKind};
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;

use support::{
    HostFixture, TestSystem, attach, collect_until, submit_input_when_node_is_online,
    submit_when_node_is_online, wait_for_path,
};

fn steer(text: &str) -> CommandInput {
    CommandInput::Text {
        text: text.to_owned(),
        context: None,
        author: Author::Principal,
        quiet: false,
        steer: true,
        attachments: Vec::new(),
    }
}

fn execution_kinds(events: &[TaskEvent], command_id: CommandId) -> Vec<ExecutionEventKind> {
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
async fn a_message_sent_while_a_turn_runs_joins_it_before_its_next_model_call() {
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
        attach(&mut surface, system.task_id).await;

        let running = CommandId::new();
        submit_when_node_is_online(
            &mut surface,
            system.task_id,
            running,
            "Render, then listen.",
        )
        .await;
        wait_for_path(&fixture.started()).await;
        // A second connection, so this reply is not behind the first's events.
        let mut sender = system.connect_surface().await;
        let steered = CommandId::new();
        submit_input_when_node_is_online(
            &mut sender,
            system.task_id,
            steered,
            steer("Keep it short."),
        )
        .await;
        // The node offers the steer while the first model call still runs.
        while !fixture.steer_recorded(system.task_id, steered) {
            sleep(Duration::from_millis(10)).await;
        }
        fixture.release();

        let ended = std::cell::Cell::new(0);
        let events = collect_until(&mut surface, |event| {
            if matches!(event.kind, ExecutionEventKind::ExecutionTerminated { .. }) {
                ended.set(ended.get() + 1);
            }
            ended.get() == 2
        })
        .await;
        let running_kinds = execution_kinds(&events, running);
        assert!(
            matches!(
                &running_kinds[running_kinds.len() - 2..],
                [
                    ExecutionEventKind::AssistantMessage { text },
                    ExecutionEventKind::ExecutionTerminated {
                        terminal: ExecutionTerminal::Completed
                    },
                ] if text == "Kept it short."
            ),
            "the running turn answered the steer: {running_kinds:?}"
        );
        assert_eq!(
            execution_kinds(&events, steered),
            vec![
                ExecutionEventKind::ExecutionStarted,
                ExecutionEventKind::ExecutionTerminated {
                    terminal: ExecutionTerminal::Completed
                },
            ],
            "the steer ran no turn of its own"
        );

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        assert_eq!(fixture.operation_count(), 1);
        system.stop().await;
    })
    .await
    .expect("steering finishes in time");
}

#[tokio::test]
async fn a_message_sent_during_the_last_model_call_runs_as_its_own_turn() {
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
        attach(&mut surface, system.task_id).await;

        let running = CommandId::new();
        submit_when_node_is_online(
            &mut surface,
            system.task_id,
            running,
            "Read proof, then wait.",
        )
        .await;
        // The model's answer, after the tool, waits for release: no model
        // call follows it, so nothing takes a steer in.
        wait_for_path(&fixture.started()).await;
        let mut sender = system.connect_surface().await;
        let late = CommandId::new();
        submit_input_when_node_is_online(&mut sender, system.task_id, late, steer("Too late."))
            .await;
        while !fixture.steer_recorded(system.task_id, late) {
            sleep(Duration::from_millis(10)).await;
        }
        fixture.release();

        let ended = std::cell::Cell::new(0);
        let events = collect_until(&mut surface, |event| {
            if matches!(event.kind, ExecutionEventKind::ExecutionTerminated { .. }) {
                ended.set(ended.get() + 1);
            }
            ended.get() == 2
        })
        .await;
        let late_kinds = execution_kinds(&events, late);
        assert!(
            late_kinds.contains(&ExecutionEventKind::TurnStarted)
                && late_kinds.contains(&ExecutionEventKind::AssistantMessage {
                    text: "Answered on its own.".to_owned()
                }),
            "the late message ran as its own turn: {late_kinds:?}"
        );

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        assert_eq!(fixture.operation_count(), 2);
        system.stop().await;
    })
    .await
    .expect("the late message finishes in time");
}

#[tokio::test]
async fn a_steer_stopped_before_it_is_offered_never_joins_the_turn() {
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
        attach(&mut surface, system.task_id).await;

        let running = CommandId::new();
        submit_when_node_is_online(
            &mut surface,
            system.task_id,
            running,
            "Render, then listen.",
        )
        .await;
        wait_for_path(&fixture.started()).await;
        let stopped = CommandId::new();
        // Recorded as a stop that reached the node ahead of its command.
        fixture.record_stop_in_ledger(system.task_id, stopped);
        let mut sender = system.connect_surface().await;
        submit_input_when_node_is_online(
            &mut sender,
            system.task_id,
            stopped,
            steer("Keep it short."),
        )
        .await;
        while !fixture.ledger_has_command(stopped) {
            sleep(Duration::from_millis(10)).await;
        }
        fixture.release();

        let ended = std::cell::Cell::new(0);
        let events = collect_until(&mut surface, |event| {
            if matches!(event.kind, ExecutionEventKind::ExecutionTerminated { .. }) {
                ended.set(ended.get() + 1);
            }
            ended.get() == 2
        })
        .await;
        assert!(
            execution_kinds(&events, running).contains(&ExecutionEventKind::AssistantMessage {
                text: "Rendered without changes.".to_owned()
            }),
            "the running turn never saw the stopped steer"
        );
        assert!(matches!(
            execution_kinds(&events, stopped).last(),
            Some(ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Cancelled { .. }
            })
        ));
        assert!(!fixture.steer_recorded(system.task_id, stopped));

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    })
    .await
    .expect("the stopped steer finishes in time");
}

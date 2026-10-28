#[allow(
    dead_code,
    reason = "the shared node fixture exposes helpers used by the other suites"
)]
mod support;

use std::time::Duration;

use renoa_control::{StopOutcome, TaskEvent, TaskEventKind};
use renoa_kernel::OperationStatus;
use renoa_node::RenoaNode;
use renoa_protocol::{CommandId, ExecutionEventKind, ExecutionTerminal, LiveActivity};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use support::{
    HostFixture, TestSystem, attach, attach_live, collect_live_until_text,
    collect_through_terminal, collect_through_turn_started, stop_command,
    submit_when_node_is_online, wait_for_path,
};

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

/// Waits for the model process to be gone: a stop must end the process the
/// model call runs in, not only the turn waiting on it.
async fn assert_stopped(pid: u32) {
    timeout(Duration::from_secs(5), async {
        while std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .expect("run kill")
            .success()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the model process kept running after the stop");
}

fn delta(sequence: u64, text: &str) -> LiveActivity {
    LiveActivity::TextDelta {
        sequence,
        text: text.to_owned(),
    }
}

#[tokio::test]
async fn streamed_text_reaches_a_live_surface_while_the_turn_runs_and_is_never_stored() {
    timeout(Duration::from_secs(15), async {
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

        let mut live = system.connect_surface().await;
        attach_live(&mut live, system.task_id).await;
        let mut sender = system.connect_surface().await;
        let command_id = CommandId::new();
        submit_when_node_is_online(&mut sender, system.task_id, command_id, "Stream an answer.")
            .await;

        // The model withholds its answer until released, so text can only
        // reach the surface while the turn runs.
        let (frames, mut events) = collect_live_until_text(&mut live, "Hello world").await;
        assert_eq!(
            frames,
            vec![delta(0, "Hello "), delta(1, "world")],
            "the reasoning delta and nothing else stayed on the node"
        );
        assert!(
            !execution_kinds(&events, command_id)
                .iter()
                .any(|kind| matches!(kind, ExecutionEventKind::ExecutionTerminated { .. })),
            "the turn is still running"
        );

        fixture.release();
        events.extend(collect_through_terminal(&mut live).await);

        let kinds = execution_kinds(&events, command_id);
        let messages: Vec<_> = kinds
            .iter()
            .filter_map(|kind| match kind {
                ExecutionEventKind::AssistantMessage { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            messages,
            ["Hello world"],
            "only the whole message is stored"
        );
        assert!(matches!(
            kinds.last(),
            Some(ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Completed
            })
        ));
        let stored = serde_json::to_string(&events).expect("serialize the journal");
        assert!(
            !stored.contains("private deliberation"),
            "reasoning reached the journal"
        );

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    })
    .await
    .expect("streaming test timed out");
}

#[tokio::test]
async fn stopping_a_running_turn_ends_it_cancelled_and_leaves_the_next_command_alone() {
    Box::pin(timeout(Duration::from_secs(15), async {
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
        let mut sender = system.connect_surface().await;
        let command_id = CommandId::new();
        submit_when_node_is_online(
            &mut sender,
            system.task_id,
            command_id,
            "Hold through reconnect.",
        )
        .await;
        wait_for_path(&fixture.started()).await;
        collect_through_turn_started(&mut surface).await;

        assert_eq!(
            stop_command(&system, system.task_id, command_id).await,
            StopOutcome::Requested
        );
        let stopped = collect_through_terminal(&mut surface).await;
        assert!(
            matches!(
                execution_kinds(&stopped, command_id).last(),
                Some(ExecutionEventKind::ExecutionTerminated {
                    terminal: ExecutionTerminal::Cancelled { .. }
                })
            ),
            "{stopped:?}"
        );
        assert_eq!(
            fixture.operation_statuses_for(system.task_id),
            [OperationStatus::Cancelled],
            "the kernel closed the turn as cancelled"
        );
        assert_stopped(fixture.model_pid()).await;
        assert_eq!(
            stop_command(&system, system.task_id, command_id).await,
            StopOutcome::Requested,
            "a retry reads the stop it recorded"
        );

        // The stop named one command: the task's next one runs to the end.
        let next = CommandId::new();
        submit_when_node_is_online(&mut sender, system.task_id, next, "First.").await;
        let events = collect_through_terminal(&mut surface).await;
        let kinds = execution_kinds(&events, next);
        assert!(kinds.iter().any(|kind| matches!(
            kind,
            ExecutionEventKind::AssistantMessage { text } if text == "First response."
        )));
        assert!(matches!(
            kinds.last(),
            Some(ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Completed
            })
        ));

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    }))
    .await
    .expect("stop test timed out");
}

#[tokio::test]
async fn a_stop_requested_while_the_node_is_down_cancels_the_turn_it_redrives() {
    Box::pin(timeout(Duration::from_secs(15), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let node_credentials = system.enroll_node().await;
        let first_shutdown = CancellationToken::new();
        let first = RenoaNode::open(system.url.clone(), node_credentials.clone(), fixture.host())
            .expect("open first execution node");
        let first_task = tokio::spawn(first.run(first_shutdown.clone()));

        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        let mut sender = system.connect_surface().await;
        let command_id = CommandId::new();
        submit_when_node_is_online(
            &mut sender,
            system.task_id,
            command_id,
            "Hold through reconnect.",
        )
        .await;
        wait_for_path(&fixture.started()).await;
        collect_through_turn_started(&mut surface).await;

        first_shutdown.cancel();
        first_task
            .await
            .expect("first node task")
            .expect("first node stops without settling active work");
        assert_eq!(
            stop_command(&system, system.task_id, command_id).await,
            StopOutcome::Requested,
            "a stop is accepted while the node is down"
        );

        // The restarted node redrives the unfinished turn and is delivered
        // the stop, in either order.
        let restarted_shutdown = CancellationToken::new();
        let restarted = RenoaNode::open(system.url.clone(), node_credentials, fixture.host())
            .expect("reopen execution node");
        let restarted_task = tokio::spawn(restarted.run(restarted_shutdown.clone()));

        let stopped = collect_through_terminal(&mut surface).await;
        assert!(
            matches!(
                execution_kinds(&stopped, command_id).last(),
                Some(ExecutionEventKind::ExecutionTerminated {
                    terminal: ExecutionTerminal::Cancelled { .. }
                })
            ),
            "{stopped:?}"
        );

        restarted_shutdown.cancel();
        restarted_task
            .await
            .expect("restarted node task")
            .expect("restarted node shuts down cleanly");
        system.stop().await;
    }))
    .await
    .expect("stop across a restart test timed out");
}

#[tokio::test]
async fn a_command_whose_stop_the_ledger_holds_starts_cancelled_and_never_reaches_the_model() {
    timeout(Duration::from_secs(15), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            fixture.host(),
        )
        .expect("open execution node");
        // The stop reached the node before the command did.
        let command_id = CommandId::new();
        fixture.record_stop_in_ledger(system.task_id, command_id);
        let node_task = tokio::spawn(node.run(node_shutdown.clone()));

        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        let mut sender = system.connect_surface().await;
        submit_when_node_is_online(
            &mut sender,
            system.task_id,
            command_id,
            "Hold through reconnect.",
        )
        .await;

        let events = collect_through_terminal(&mut surface).await;
        assert!(
            matches!(
                execution_kinds(&events, command_id).last(),
                Some(ExecutionEventKind::ExecutionTerminated {
                    terminal: ExecutionTerminal::Cancelled { .. }
                })
            ),
            "{events:?}"
        );
        assert!(!fixture.started().exists(), "the model was called");

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    })
    .await
    .expect("stop before start test timed out");
}

#[tokio::test]
async fn a_queued_command_that_is_stopped_ends_cancelled_without_a_model_call() {
    Box::pin(timeout(Duration::from_secs(15), async {
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
        let mut sender = system.connect_surface().await;
        let first = CommandId::new();
        submit_when_node_is_online(
            &mut sender,
            system.task_id,
            first,
            "Hold through reconnect.",
        )
        .await;
        wait_for_path(&fixture.started()).await;
        collect_through_turn_started(&mut surface).await;

        // The second command waits behind the first in the task's session.
        let second = CommandId::new();
        submit_when_node_is_online(&mut sender, system.task_id, second, "Second.").await;
        assert_eq!(
            stop_command(&system, system.task_id, second).await,
            StopOutcome::Requested
        );
        fixture.release();

        let first_turn = collect_through_terminal(&mut surface).await;
        assert!(
            matches!(
                execution_kinds(&first_turn, first).last(),
                Some(ExecutionEventKind::ExecutionTerminated {
                    terminal: ExecutionTerminal::Completed
                })
            ),
            "the stop named the second command only"
        );
        let second_turn = execution_kinds(&collect_through_terminal(&mut surface).await, second);
        assert!(
            matches!(
                second_turn.last(),
                Some(ExecutionEventKind::ExecutionTerminated {
                    terminal: ExecutionTerminal::Cancelled { .. }
                })
            ),
            "{second_turn:?}"
        );
        assert!(
            !second_turn.iter().any(|kind| matches!(
                kind,
                ExecutionEventKind::AssistantMessage { text } if text == "Second response."
            )),
            "the stopped command was answered"
        );

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    }))
    .await
    .expect("queued stop test timed out");
}

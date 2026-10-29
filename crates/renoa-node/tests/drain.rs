#[allow(
    dead_code,
    reason = "the shared node fixture exposes helpers used by the other suites"
)]
mod support;

use std::time::Duration;

use renoa_control::{StopOutcome, TaskEvent, TaskEventKind};
use renoa_kernel::OperationStatus;
use renoa_node::RenoaNode;
use renoa_protocol::{CommandId, ExecutionEventKind, ExecutionTerminal};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use support::{
    CuttableProxy, HostFixture, TestSystem, attach, collect_through_terminal,
    collect_through_turn_started, stop_on, submit_when_node_is_online, wait_for_path,
};

/// How the command ended, once its terminal record has arrived.
fn terminal(events: &[TaskEvent], command_id: CommandId) -> Option<ExecutionTerminal> {
    events.iter().find_map(|event| match &event.kind {
        TaskEventKind::ExecutionEvent {
            command_id: cause,
            event,
        } if *cause == command_id => match &event.kind {
            ExecutionEventKind::ExecutionTerminated { terminal } => Some(terminal.clone()),
            _ => None,
        },
        _ => None,
    })
}

/// The deploy's drain: the turn the node took finishes under the runtime it
/// was admitted with, then the node stops by itself.
#[tokio::test]
async fn a_draining_node_finishes_the_turn_it_took_and_stops() {
    Box::pin(timeout(Duration::from_secs(20), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let drain = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            fixture.host(),
        )
        .expect("open execution node")
        .with_drain(drain.clone());
        let node_task = tokio::spawn(node.run(CancellationToken::new()));

        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        let mut sender = system.connect_surface().await;
        let running = CommandId::new();
        submit_when_node_is_online(
            &mut sender,
            system.task_id,
            running,
            "Hold through reconnect.",
        )
        .await;
        wait_for_path(&fixture.started()).await;
        collect_through_turn_started(&mut surface).await;

        drain.cancel();
        fixture.release();
        node_task
            .await
            .expect("draining node task")
            .expect("the drained node stops by itself, with no shutdown");
        assert_eq!(
            fixture.operation_statuses_for(system.task_id),
            [OperationStatus::Completed],
            "the running turn finished"
        );
        system.stop().await;
    }))
    .await
    .expect("drain test timed out");
}

/// A command that reaches a draining node is not admitted, so the
/// coordinator keeps it and the node that replaces this one runs it.
#[tokio::test]
async fn a_command_sent_to_a_draining_node_runs_on_the_next_node() {
    Box::pin(timeout(Duration::from_secs(20), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let credentials = system.enroll_node().await;
        let drain = CancellationToken::new();
        let draining = RenoaNode::open(system.url.clone(), credentials.clone(), fixture.host())
            .expect("open execution node")
            .with_drain(drain.clone());
        let draining_task = tokio::spawn(draining.run(CancellationToken::new()));

        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        let mut sender = system.connect_surface().await;
        let running = CommandId::new();
        submit_when_node_is_online(
            &mut sender,
            system.task_id,
            running,
            "Hold through reconnect.",
        )
        .await;
        wait_for_path(&fixture.started()).await;
        collect_through_turn_started(&mut surface).await;

        drain.cancel();
        let queued = CommandId::new();
        submit_when_node_is_online(&mut sender, system.task_id, queued, "Second.").await;
        // Asked on the same connection, so the node reads the queued command
        // before this stop, which ends the turn it holds open.
        assert_eq!(
            stop_on(&mut sender, system.task_id, running).await,
            StopOutcome::Requested
        );
        draining_task
            .await
            .expect("draining node task")
            .expect("the drained node stops by itself, with no shutdown");
        assert_eq!(
            fixture.operation_statuses_for(system.task_id),
            [OperationStatus::Cancelled],
            "the draining node did not take the queued command"
        );

        let shutdown = CancellationToken::new();
        let replacement = RenoaNode::open(system.url.clone(), credentials, fixture.host())
            .expect("reopen execution node");
        let replacement_task = tokio::spawn(replacement.run(shutdown.clone()));
        let mut events = Vec::new();
        while terminal(&events, queued).is_none() {
            events.extend(collect_through_terminal(&mut surface).await);
        }
        assert_eq!(
            terminal(&events, queued),
            Some(ExecutionTerminal::Completed)
        );
        assert_eq!(
            fixture.operation_statuses_for(system.task_id),
            [OperationStatus::Cancelled, OperationStatus::Completed],
            "the queued command ran on the replacement"
        );

        shutdown.cancel();
        replacement_task
            .await
            .expect("replacement node task")
            .expect("replacement node shuts down cleanly");
        system.stop().await;
    }))
    .await
    .expect("deferral test timed out");
}

#[tokio::test]
async fn an_idle_node_stops_as_soon_as_it_drains() {
    timeout(Duration::from_secs(10), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let drain = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            system.enroll_node().await,
            fixture.host(),
        )
        .expect("open execution node")
        .with_drain(drain.clone());
        let node_task = tokio::spawn(node.run(CancellationToken::new()));
        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        let mut sender = system.connect_surface().await;
        let command_id = CommandId::new();
        submit_when_node_is_online(&mut sender, system.task_id, command_id, "Read proof.").await;
        collect_through_terminal(&mut surface).await;

        drain.cancel();
        node_task
            .await
            .expect("node task")
            .expect("an idle node stops when it drains");
        system.stop().await;
    })
    .await
    .expect("idle drain test timed out");
}

#[tokio::test]
async fn a_node_that_cannot_reach_the_coordinator_still_stops_when_it_drains() {
    timeout(Duration::from_secs(10), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let proxy = CuttableProxy::start(system.url.clone()).await;
        let drain = CancellationToken::new();
        let node = RenoaNode::open(
            proxy.url.clone(),
            system.enroll_node().await,
            fixture.host(),
        )
        .expect("open execution node")
        .with_drain(drain.clone());
        let node_task = tokio::spawn(node.run(CancellationToken::new()));
        // From here every connection attempt is refused.
        proxy.stop().await;

        drain.cancel();
        node_task
            .await
            .expect("node task")
            .expect("a draining node stops without the coordinator");
        system.stop().await;
    })
    .await
    .expect("unreachable drain test timed out");
}

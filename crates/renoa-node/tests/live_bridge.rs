mod support;

use std::{sync::Arc, time::Duration};

use renoa_control::TaskEventKind;
use renoa_core::CommandId;
use renoa_node::RenoaNode;
use renoa_protocol::{ExecutionEventKind, ExecutionTerminal};
use renoa_runtime::EngineConfig;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use support::{
    CuttableProxy, GatedModel, NoCapabilities, TestSystem, attach, collect_through_model_request,
    collect_through_terminal, submit_when_node_is_online, test_agent,
};

#[tokio::test]
async fn a_surface_observes_durable_execution_events_while_the_model_is_still_running() {
    timeout(Duration::from_secs(5), async {
        let system = TestSystem::start().await;
        let node_credentials = system.enroll_node().await;
        let model = Arc::new(GatedModel::new());
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            system.url.clone(),
            node_credentials,
            system.files.path().join("node.sqlite"),
            test_agent(),
            model.clone(),
            Arc::new(NoCapabilities),
            EngineConfig::default(),
        )
        .expect("open execution node");
        let node_task = tokio::spawn(node.run(node_shutdown.clone()));

        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        let command_id = CommandId::new();
        submit_when_node_is_online(&mut surface, system.task_id, command_id).await;
        model.wait_until_requested().await;

        let live_events = collect_through_model_request(&mut surface).await;
        assert!(matches!(
            live_events.first().map(|event| &event.kind),
            Some(TaskEventKind::CommandSubmitted { command })
                if command.command_id == command_id
        ));
        assert!(live_events.iter().any(|event| matches!(
            &event.kind,
            TaskEventKind::ExecutionEvent { event }
                if matches!(event.kind, ExecutionEventKind::ExecutionStarted)
        )));
        assert!(matches!(
            live_events.last().map(|event| &event.kind),
            Some(TaskEventKind::ExecutionEvent { event })
                if matches!(event.kind, ExecutionEventKind::TurnStarted)
        ));

        model.release();
        let terminal_events = collect_through_terminal(&mut surface).await;
        assert!(terminal_events.iter().any(|event| matches!(
            &event.kind,
            TaskEventKind::ExecutionEvent { event }
                if matches!(event.kind, ExecutionEventKind::AssistantMessage { .. })
        )));

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    })
    .await
    .expect("live bridge test timed out");
}

#[tokio::test]
async fn a_restarted_node_closes_an_interrupted_run_without_repeating_it() {
    timeout(Duration::from_secs(5), async {
        let system = TestSystem::start().await;
        let node_credentials = system.enroll_node().await;
        let node_path = system.files.path().join("node.sqlite");
        let model = Arc::new(GatedModel::new());
        let node = RenoaNode::open(
            system.url.clone(),
            node_credentials.clone(),
            &node_path,
            test_agent(),
            model.clone(),
            Arc::new(NoCapabilities),
            EngineConfig::default(),
        )
        .expect("open first execution node");
        let first_node = tokio::spawn(node.run(CancellationToken::new()));

        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        let command_id = CommandId::new();
        submit_when_node_is_online(&mut surface, system.task_id, command_id).await;
        model.wait_until_requested().await;
        collect_through_model_request(&mut surface).await;

        first_node.abort();
        assert!(
            first_node
                .await
                .expect_err("simulate node process crash")
                .is_cancelled()
        );

        let restarted_shutdown = CancellationToken::new();
        let restarted = RenoaNode::open(
            system.url.clone(),
            node_credentials,
            node_path,
            test_agent(),
            Arc::new(GatedModel::new()),
            Arc::new(NoCapabilities),
            EngineConfig::default(),
        )
        .expect("reopen execution node");
        let restarted_task = tokio::spawn(restarted.run(restarted_shutdown.clone()));

        let terminal_events = collect_through_terminal(&mut surface).await;
        assert!(matches!(
            terminal_events.last().map(|event| &event.kind),
            Some(TaskEventKind::ExecutionEvent { event })
                if matches!(
                    &event.kind,
                    ExecutionEventKind::ExecutionTerminated {
                        terminal: ExecutionTerminal::Failed { error }
                    } if error == "execution interrupted by node restart"
                )
        ));

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
async fn transport_reconnect_does_not_interrupt_the_running_engine() {
    timeout(Duration::from_secs(5), async {
        let system = TestSystem::start().await;
        let proxy = CuttableProxy::start(system.url.clone()).await;
        let node_credentials = system.enroll_node().await;
        let model = Arc::new(GatedModel::new());
        let node_shutdown = CancellationToken::new();
        let node = RenoaNode::open(
            proxy.url.clone(),
            node_credentials,
            system.files.path().join("node.sqlite"),
            test_agent(),
            model.clone(),
            Arc::new(NoCapabilities),
            EngineConfig::default(),
        )
        .expect("open execution node");
        let node_task = tokio::spawn(node.run(node_shutdown.clone()));

        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        let command_id = CommandId::new();
        submit_when_node_is_online(&mut surface, system.task_id, command_id).await;
        model.wait_until_requested().await;
        collect_through_model_request(&mut surface).await;

        proxy.cut().await;
        model.release();

        let terminal_events = timeout(
            Duration::from_secs(2),
            collect_through_terminal(&mut surface),
        )
        .await
        .expect("node publishes the terminal suffix after reconnect");
        assert!(matches!(
            terminal_events.last().map(|event| &event.kind),
            Some(TaskEventKind::ExecutionEvent { event })
                if matches!(
                    &event.kind,
                    ExecutionEventKind::ExecutionTerminated {
                        terminal: ExecutionTerminal::Completed
                    }
                )
        ));
        assert!(terminal_events.iter().any(|event| matches!(
            &event.kind,
            TaskEventKind::ExecutionEvent { event }
                if matches!(
                    &event.kind,
                    ExecutionEventKind::AssistantMessage { text }
                        if text == "finished live"
                )
        )));

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

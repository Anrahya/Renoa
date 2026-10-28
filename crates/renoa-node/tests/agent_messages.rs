//! An agent messages another agent of its owner through the node's coordinator
//! link, using the Host tool a model discovers and calls, and the answer
//! returns to the asker's conversation.

#[allow(
    dead_code,
    unused_imports,
    reason = "the shared node fixture exposes helpers used by other suites"
)]
mod support;

use std::time::Duration;

use renoa_control::{TaskEvent, TaskEventKind};
use renoa_node::RenoaNode;
use renoa_protocol::{AgentMessageKind, CommandEnvelope, CommandId, ExecutionEventKind};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use support::{
    HostFixture, TestSystem, agent_target, attach, attach_after, collect_through_terminal,
    list_tasks, submit_when_node_is_online, wait_for_targets,
};

#[tokio::test]
async fn an_agent_messages_another_and_the_answer_returns_to_its_conversation() {
    Box::pin(timeout(Duration::from_secs(30), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let beta = fixture.provision_agent_named("Beta").await;
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
        attach(&mut surface, system.task_id).await;

        let asked = CommandId::new();
        submit_when_node_is_online(
            &mut surface,
            system.task_id,
            asked,
            "Ask Beta: Count the files.",
        )
        .await;
        // The turn that asks, then the turn that reads the answer.
        let mut events = collect_through_terminal(&mut surface).await;
        events.extend(collect_through_terminal(&mut surface).await);

        let sent = tool_finished(&events, asked, "send-message");
        assert!(!sent.1, "{}", sent.0);
        assert!(sent.0.starts_with("Sent to Beta."), "{}", sent.0);
        assert!(
            execution_events(&events, asked)
                .any(|kind| matches!(kind, ExecutionEventKind::AssistantMessage { text } if text == "Asked.")),
            "{events:#?}"
        );

        let reply = submitted(&events)
            .into_iter()
            .find(|command| command.origin.is_some())
            .expect("the answer arrives as a new message in the asker's conversation");
        let origin = reply.origin.clone().expect("origin");
        assert_eq!(origin.kind, AgentMessageKind::Reply);
        assert_eq!(origin.name, "Beta");
        assert_eq!(origin.from, agent_target(beta));
        assert_eq!(origin.depth, 2);
        assert_eq!(
            reply.input.text(),
            "Count: 3. Message from the agent Alpha. Your answer is sent back to them automatically."
        );
        assert!(
            execution_events(&events, reply.command_id).any(|kind| matches!(
                kind,
                ExecutionEventKind::AssistantMessage { text }
                    if text == "Thanks. Reply from the agent Beta to a message you sent."
            )),
            "{events:#?}"
        );

        // The receiving agent holds the exchange in a thread the owner sees.
        let tasks = list_tasks(&system).await;
        let thread = tasks
            .iter()
            .find(|task| task.target == agent_target(beta))
            .expect("Beta has a thread for the message");
        let mut reader = system.connect_surface().await;
        // The thread already ended its turn, so a full replay holds all of it.
        attach_after(&mut reader, thread.task_id, None)
            .await
            .expect("the thread holds the exchange");
        let thread_events = collect_through_terminal(&mut reader).await;
        let request = submitted(&thread_events)
            .into_iter()
            .next()
            .expect("the message is the thread's first command");
        assert_eq!(
            reply.origin.expect("origin").from_command,
            request.command_id,
            "the answer names the request it answers"
        );
        let origin = request.origin.expect("the request names its sender");
        assert_eq!(origin.kind, AgentMessageKind::Request);
        assert_eq!(origin.from_command, asked);
        assert_eq!(origin.name, "Alpha");
        assert_eq!(origin.from, agent_target(fixture.agent_id));
        assert_eq!(origin.depth, 1);
        assert_eq!(request.input.text(), "Count the files.");

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    }))
    .await
    .expect("agent messaging test timed out");
}

#[tokio::test]
async fn a_wrong_name_is_answered_with_the_agents_there_are_and_a_correct_call() {
    timeout(Duration::from_secs(30), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        fixture.provision_agent_named("Beta").await;
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
        attach(&mut surface, system.task_id).await;

        let asked = CommandId::new();
        submit_when_node_is_online(
            &mut surface,
            system.task_id,
            asked,
            "Ask Nobody: Count the files.",
        )
        .await;
        let events = collect_through_terminal(&mut surface).await;

        let (output, is_error) = tool_finished(&events, asked, "send-message");
        assert!(is_error, "{output}");
        assert!(output.contains("No agent named \"Nobody\""), "{output}");
        assert!(output.contains("\"Beta\""), "{output}");
        assert!(
            !output.contains("\"Alpha\""),
            "an agent is not offered itself: {output}"
        );
        assert!(output.contains("A correct call is"), "{output}");
        assert!(
            list_tasks(&system)
                .await
                .iter()
                .all(|task| task.target == system.target),
            "a refused message opens no thread"
        );

        node_shutdown.cancel();
        node_task
            .await
            .expect("node task")
            .expect("node shuts down cleanly");
        system.stop().await;
    })
    .await
    .expect("agent messaging refusal test timed out");
}

fn submitted(events: &[TaskEvent]) -> Vec<CommandEnvelope> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            TaskEventKind::CommandSubmitted { command } => Some(command.clone()),
            TaskEventKind::ExecutionEvent { .. } => None,
        })
        .collect()
}

fn execution_events(
    events: &[TaskEvent],
    caused_by: CommandId,
) -> impl Iterator<Item = &ExecutionEventKind> {
    events.iter().filter_map(move |event| match &event.kind {
        TaskEventKind::ExecutionEvent { command_id, event } if *command_id == caused_by => {
            Some(&event.kind)
        }
        _ => None,
    })
}

/// The output and error flag of the tool call `call` in the turn of `command`.
fn tool_finished(events: &[TaskEvent], command: CommandId, call: &str) -> (String, bool) {
    execution_events(events, command)
        .find_map(|kind| match kind {
            ExecutionEventKind::ToolFinished {
                call_id,
                output,
                is_error,
            } if call_id == call => Some((output.clone(), *is_error)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("tool call {call} did not finish in {events:#?}"))
}

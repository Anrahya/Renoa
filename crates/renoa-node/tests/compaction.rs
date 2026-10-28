//! The owner's `/compact` through the real coordinator, node and Host.
//!
//! RCP carries `/compact` as text; the node decides it is a control. The fake
//! model rejects any request whose tool calls and results do not pair, so every
//! turn here also proves compaction left the transcript valid for a provider.

#[allow(
    dead_code,
    unused_imports,
    reason = "the shared node fixture exposes helpers used by the other node suites"
)]
mod support;

use std::time::Duration;

use renoa_control::{DeviceCredentials, TaskEvent, TaskEventKind, TaskId};
use renoa_node::{NodeError, RenoaNode};
use renoa_protocol::{Author, CommandId, CommandInput, ExecutionEventKind, ExecutionTerminal};
use tokio::{task::JoinHandle, time::timeout};
use tokio_util::sync::CancellationToken;

use support::{
    HostFixture, Socket, TestSystem, attach, collect_through_terminal,
    collect_through_turn_started, submit_input_when_node_is_online, submit_when_node_is_online,
    wait_for_path,
};

const MESSAGE: &str = "renoa.agent.message.v1";
const CHECKPOINT: &str = "renoa.agent.context-checkpoint.v1";
const NOTICE: &str =
    "Earlier messages are summarized now. This conversation continues from that summary.";

struct Running {
    shutdown: CancellationToken,
    task: JoinHandle<Result<(), NodeError>>,
}

fn start_node(
    system: &TestSystem,
    credentials: DeviceCredentials,
    fixture: &HostFixture,
) -> Running {
    let shutdown = CancellationToken::new();
    let node = RenoaNode::open(system.url.clone(), credentials, fixture.host())
        .expect("open execution node");
    Running {
        task: tokio::spawn(node.run(shutdown.clone())),
        shutdown,
    }
}

impl Running {
    async fn stop(self) {
        self.shutdown.cancel();
        self.task
            .await
            .expect("node task")
            .expect("node stops cleanly");
    }
}

/// What the command's execution recorded, in order.
fn recorded(events: &[TaskEvent], command_id: CommandId) -> Vec<ExecutionEventKind> {
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

/// Sends the owner's `text` and waits for its execution to end.
async fn say(
    surface: &mut Socket,
    task_id: TaskId,
    text: &str,
) -> (CommandId, Vec<ExecutionEventKind>) {
    let command_id = CommandId::new();
    submit_when_node_is_online(surface, task_id, command_id, text).await;
    let events = collect_through_terminal(surface).await;
    (command_id, recorded(&events, command_id))
}

fn completed() -> ExecutionEventKind {
    ExecutionEventKind::ExecutionTerminated {
        terminal: ExecutionTerminal::Completed,
    }
}

fn notice() -> ExecutionEventKind {
    ExecutionEventKind::AssistantMessage {
        text: NOTICE.to_owned(),
    }
}

#[tokio::test]
async fn the_owners_compact_summarizes_the_chat_and_the_next_turn_continues_from_the_summary() {
    timeout(Duration::from_secs(20), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let node = start_node(&system, system.enroll_node().await, &fixture);
        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;

        // One turn with a tool call and its result, so the summarized prefix
        // holds a call and its result together.
        say(&mut surface, system.task_id, "Read proof.").await;
        assert_eq!(fixture.session_event_count(system.task_id, MESSAGE), 4);

        let (_, compact) = say(&mut surface, system.task_id, "/compact").await;
        assert_eq!(
            compact,
            vec![
                ExecutionEventKind::ExecutionStarted,
                ExecutionEventKind::TurnStarted,
                notice(),
                completed(),
            ],
            "the owner sees one notice and the agent says nothing"
        );
        assert_eq!(fixture.summary_attempts(), 1);
        let request: serde_json::Value =
            serde_json::from_str(&fixture.summary_request()).expect("summary request is JSON");
        let summarized = request["messages"][0]["content"][0]["text"]
            .as_str()
            .expect("the summary request's transcript")
            .to_owned();
        // The transcript holds one JSON line per message, so the file's
        // newline appears escaped inside the tool result line.
        for expected in ["Read proof.", "read-proof", "durable proof\\n"] {
            assert!(
                summarized.contains(expected),
                "the summary request lacks {expected:?}: {summarized}"
            );
        }
        assert!(
            !summarized.contains("/compact"),
            "the control is not part of the conversation"
        );
        assert_eq!(fixture.session_event_count(system.task_id, CHECKPOINT), 1);
        assert_eq!(
            fixture.session_event_count(system.task_id, MESSAGE),
            4,
            "compaction adds no message and deletes none"
        );
        assert_eq!(fixture.operation_count(), 2);

        let (_, second) = say(&mut surface, system.task_id, "Second.").await;
        assert!(second.contains(&ExecutionEventKind::AssistantMessage {
            text: "Second response.".to_owned()
        }));
        let continued = fixture.last_request().expect("the second turn's request");
        assert!(continued.contains("[CONTEXT CHECKPOINT]"));
        assert!(continued.contains("Second."));
        assert!(
            !continued.contains("The durable proof was read."),
            "the summarized turn is no longer sent verbatim"
        );
        assert_eq!(
            fixture.session_event_count(system.task_id, MESSAGE),
            6,
            "every original message is still recorded"
        );

        node.stop().await;
        system.stop().await;
    })
    .await
    .expect("owner compaction test timed out");
}

#[tokio::test]
async fn compacting_an_empty_chat_asks_no_model() {
    timeout(Duration::from_secs(20), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let node = start_node(&system, system.enroll_node().await, &fixture);
        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;

        let (_, compact) = say(&mut surface, system.task_id, "  /compact\n").await;

        assert_eq!(compact.last(), Some(&completed()));
        assert_eq!(fixture.summary_attempts(), 0);
        assert!(fixture.last_request().is_none(), "no model was called");
        assert_eq!(fixture.session_event_count(system.task_id, CHECKPOINT), 0);

        node.stop().await;
        system.stop().await;
    })
    .await
    .expect("empty compaction test timed out");
}

#[tokio::test]
async fn a_guests_compact_is_a_message_for_the_agent_and_summarizes_nothing() {
    timeout(Duration::from_secs(20), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let node = start_node(&system, system.enroll_node().await, &fixture);
        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        say(&mut surface, system.task_id, "First.").await;

        let command_id = CommandId::new();
        submit_input_when_node_is_online(
            &mut surface,
            system.task_id,
            command_id,
            CommandInput::Text {
                text: "/compact".to_owned(),
                context: None,
                author: Author::Guest,
            },
        )
        .await;
        let recorded = recorded(&collect_through_terminal(&mut surface).await, command_id);

        assert!(recorded.contains(&ExecutionEventKind::AssistantMessage {
            text: "A guest asked: /compact".to_owned()
        }));
        assert!(!recorded.contains(&notice()));
        assert_eq!(fixture.summary_attempts(), 0);
        assert_eq!(fixture.session_event_count(system.task_id, CHECKPOINT), 0);

        node.stop().await;
        system.stop().await;
    })
    .await
    .expect("guest compaction test timed out");
}

#[tokio::test]
async fn compact_with_extra_words_fails_before_any_model_or_session_work() {
    timeout(Duration::from_secs(20), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let node = start_node(&system, system.enroll_node().await, &fixture);
        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;

        let (_, refused) = say(&mut surface, system.task_id, "/compact now").await;

        let [
            ExecutionEventKind::ExecutionStarted,
            ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Failed { error },
            },
        ] = refused.as_slice()
        else {
            panic!("a refused command records only its start and failure: {refused:?}");
        };
        assert!(error.contains("/compact on its own"), "{error}");
        assert!(fixture.last_request().is_none(), "no model was called");
        assert!(
            !fixture.data.join("sessions").exists()
                || std::fs::read_dir(fixture.data.join("sessions"))
                    .expect("read the Host's sessions")
                    .next()
                    .is_none(),
            "no Host session was opened"
        );

        node.stop().await;
        system.stop().await;
    })
    .await
    .expect("refused compaction test timed out");
}

#[tokio::test]
async fn an_invalid_summary_fails_the_command_and_leaves_the_chat_usable() {
    timeout(Duration::from_secs(20), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let node = start_node(&system, system.enroll_node().await, &fixture);
        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        say(&mut surface, system.task_id, "Break the summary.").await;

        let (_, compact) = say(&mut surface, system.task_id, "/compact").await;

        let Some(ExecutionEventKind::ExecutionTerminated {
            terminal: ExecutionTerminal::Failed { error },
        }) = compact.last()
        else {
            panic!("an invalid summary fails the command: {compact:?}");
        };
        assert!(error.contains("compaction response"), "{error}");
        assert!(!compact.contains(&notice()));
        assert_eq!(fixture.summary_attempts(), 2, "the loop retries once");
        assert_eq!(fixture.session_event_count(system.task_id, CHECKPOINT), 0);

        let (_, next) = say(&mut surface, system.task_id, "Second.").await;
        assert!(next.contains(&ExecutionEventKind::AssistantMessage {
            text: "Second response.".to_owned()
        }));

        node.stop().await;
        system.stop().await;
    })
    .await
    .expect("invalid summary test timed out");
}

#[tokio::test]
async fn a_node_restart_during_the_summary_finishes_the_same_compaction_once() {
    timeout(Duration::from_secs(20), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let credentials = system.enroll_node().await;
        let first = start_node(&system, credentials.clone(), &fixture);
        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        say(&mut surface, system.task_id, "Hold the summary.").await;

        let command_id = CommandId::new();
        submit_when_node_is_online(&mut surface, system.task_id, command_id, "/compact").await;
        wait_for_path(&fixture.started()).await;
        collect_through_turn_started(&mut surface).await;
        first.stop().await;
        assert_eq!(
            fixture.summary_attempts(),
            1,
            "the first summary was asked and never answered"
        );

        let restarted = start_node(&system, credentials, &fixture);
        let finished = recorded(&collect_through_terminal(&mut surface).await, command_id);

        assert_eq!(finished, vec![notice(), completed()]);
        assert_eq!(fixture.summary_attempts(), 2, "the summary was asked again");
        assert_eq!(fixture.session_event_count(system.task_id, CHECKPOINT), 1);
        assert_eq!(
            fixture.operation_count(),
            2,
            "the restart resumed the compaction instead of starting another"
        );

        restarted.stop().await;
        system.stop().await;
    })
    .await
    .expect("restart during summary test timed out");
}

#[tokio::test]
async fn automatic_compaction_inside_a_turn_never_shows_its_summary_to_a_surface() {
    timeout(Duration::from_secs(30), async {
        let mut system = TestSystem::start().await;
        let fixture = HostFixture::install(&mut system).await;
        let node = start_node(&system, system.enroll_node().await, &fixture);
        let mut surface = system.connect_surface().await;
        attach(&mut surface, system.task_id).await;
        // Each message is about half of the fixture model's input budget, so
        // the second turn cannot be sent without summarizing the first.
        let filler = "x".repeat(150_000);
        say(
            &mut surface,
            system.task_id,
            &format!("Long message one. {filler}"),
        )
        .await;
        assert_eq!(fixture.summary_attempts(), 0);

        let (_, second) = say(
            &mut surface,
            system.task_id,
            &format!("Long message two. {filler}"),
        )
        .await;

        assert_eq!(
            second,
            vec![
                ExecutionEventKind::ExecutionStarted,
                ExecutionEventKind::TurnStarted,
                ExecutionEventKind::AssistantMessage {
                    text: "Long answer two.".to_owned()
                },
                completed(),
            ],
            "the journal holds the reply and not the summary written before it"
        );
        assert_eq!(fixture.summary_attempts(), 1);
        assert_eq!(fixture.session_event_count(system.task_id, CHECKPOINT), 1);
        let summarized = fixture.summary_request();
        assert!(summarized.contains("Long message one."));
        assert!(
            !summarized.contains("Long message two."),
            "the message being answered stays verbatim"
        );
        let reply = fixture.last_request().expect("the reply's request");
        assert!(reply.contains("[CONTEXT CHECKPOINT]"));
        assert!(reply.contains("Long message two."));
        assert!(
            !reply.contains("Long message one."),
            "the summarized turn is no longer sent verbatim"
        );
        assert_eq!(fixture.session_event_count(system.task_id, MESSAGE), 4);

        node.stop().await;
        system.stop().await;
    })
    .await
    .expect("automatic compaction test timed out");
}

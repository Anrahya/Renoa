use renoa_agent::ContentBlock;
use renoa_agent_loop::{AgentCommand, ContextContribution, TurnContext};
use renoa_kernel::{AgentId, Command, CommandId, SessionId};
use tempfile::tempdir;

use super::{LocalSession, LocalSessionError};
use crate::TurnObservation;

fn session(directory: &std::path::Path) -> LocalSession {
    LocalSession::create(
        directory.join("kernel.sqlite3"),
        AgentId::new(),
        SessionId::new(),
    )
    .expect("create local session")
}

fn admit(session: &LocalSession, command_id: CommandId, command: &serde_json::Value) {
    session
        .kernel
        .submit(
            session.session_id,
            Command::new(command_id, command.clone()),
        )
        .expect("admit command");
}

fn context(text: &str) -> TurnContext {
    TurnContext::new(vec![
        ContextContribution::plugin("renoa.time", text).expect("entry"),
    ])
    .expect("context")
}

#[test]
fn a_retry_reuses_the_admitted_command_without_recomputing_its_context() {
    let directory = tempdir().expect("temporary directory");
    let session = session(directory.path());
    let command_id = CommandId::new();
    let content = vec![ContentBlock::text("hello")];
    let first = session
        .prompt_command(command_id, &content, observation(1_000), |previous| {
            assert_eq!(previous, None, "the first prompt has no previous one");
            context("first")
        })
        .expect("create first command");
    admit(
        &session,
        command_id,
        &serde_json::to_value(&first).expect("encode"),
    );

    let replayed = session
        .prompt_command(command_id, &content, observation(99_000), |_| {
            panic!("a retry must not recompute its context")
        })
        .expect("recover admitted command");
    let mut seen = None;
    let next = session
        .prompt_command(CommandId::new(), &content, observation(6_000), |previous| {
            seen = previous;
            TurnContext::default()
        })
        .expect("create next command");

    assert_eq!(replayed, first);
    assert_eq!(replayed.context(), context("first").entries());
    assert_eq!(seen, Some(1_000));
    assert_eq!(next.observed_at_unix_ms(), Some(6_000));
    assert!(next.context().is_empty());
}

#[test]
fn a_command_stored_with_turn_timing_is_reused_and_times_the_next_prompt() {
    let directory = tempdir().expect("temporary directory");
    let session = session(directory.path());
    let command_id = CommandId::new();
    let stored = serde_json::json!({
        "content": [{"type": "text", "text": "hello"}],
        "turn_timing": {
            "observed_at": "1970-01-01T00:00:02Z[UTC]",
            "observed_at_unix_ms": 2_000,
        },
    });
    admit(&session, command_id, &stored);

    let replayed = session
        .prompt_command(
            command_id,
            &[ContentBlock::text("hello")],
            observation(9_000),
            |_| panic!("a retry must not recompute its context"),
        )
        .expect("recover the stored command");
    let mut seen = None;
    session
        .prompt_command(
            CommandId::new(),
            &[ContentBlock::text("next")],
            observation(5_000),
            |previous| {
                seen = previous;
                TurnContext::default()
            },
        )
        .expect("create next command");

    assert_eq!(
        serde_json::to_value(&replayed).expect("re-encode"),
        stored,
        "the stored command is reused byte for byte"
    );
    assert_eq!(seen, Some(2_000));
}

#[test]
fn reused_command_id_with_different_prompt_still_conflicts() {
    let directory = tempdir().expect("temporary directory");
    let session = session(directory.path());
    let command_id = CommandId::new();
    let first = AgentCommand::observed(
        vec![ContentBlock::text("first")],
        1_000,
        TurnContext::default(),
    )
    .expect("command");
    admit(
        &session,
        command_id,
        &serde_json::to_value(first).expect("encode"),
    );

    assert!(matches!(
        session.prompt_command(
            command_id,
            &[ContentBlock::text("different")],
            observation(2_000),
            |_| TurnContext::default(),
        ),
        Err(LocalSessionError::CommandConflict { .. })
    ));
}

fn observation(milliseconds: i64) -> TurnObservation {
    TurnObservation::from_unix_milliseconds(milliseconds).expect("valid observation")
}

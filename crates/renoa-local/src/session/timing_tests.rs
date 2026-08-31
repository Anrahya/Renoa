use renoa_agent::ContentBlock;
use renoa_kernel::{AgentId, Command, CommandId, SessionId};
use tempfile::tempdir;

use super::{LocalSession, LocalSessionError};
use crate::TurnObservation;

#[test]
fn retry_reuses_admitted_timing_and_next_turn_uses_it_for_elapsed_time() {
    let directory = tempdir().expect("temporary directory");
    let session = LocalSession::create(
        directory.path().join("kernel.sqlite3"),
        AgentId::new(),
        SessionId::new(),
    )
    .expect("create local session");
    let command_id = CommandId::new();
    let content = vec![ContentBlock::text("hello")];
    let first = session
        .observed_command(command_id, &content, observation(1_000))
        .expect("create first timed command");
    session
        .kernel
        .submit(
            session.session_id,
            Command::new(
                command_id,
                serde_json::to_value(&first).expect("encode first command"),
            ),
        )
        .expect("admit first command");

    let replayed = session
        .observed_command(command_id, &content, observation(99_000))
        .expect("recover admitted command");
    let next = session
        .observed_command(CommandId::new(), &content, observation(6_000))
        .expect("create next command");

    assert_eq!(replayed, first);
    assert_eq!(
        next.turn_timing()
            .expect("next turn timing")
            .elapsed_since_previous_user_message_ms(),
        Some(5_000)
    );
}

#[test]
fn reused_command_id_with_different_prompt_still_conflicts() {
    let directory = tempdir().expect("temporary directory");
    let session = LocalSession::create(
        directory.path().join("kernel.sqlite3"),
        AgentId::new(),
        SessionId::new(),
    )
    .expect("create local session");
    let command_id = CommandId::new();
    let first = session
        .observed_command(
            command_id,
            &[ContentBlock::text("first")],
            observation(1_000),
        )
        .expect("create first command");
    session
        .kernel
        .submit(
            session.session_id,
            Command::new(
                command_id,
                serde_json::to_value(first).expect("encode command"),
            ),
        )
        .expect("admit command");

    assert!(matches!(
        session.observed_command(
            command_id,
            &[ContentBlock::text("different")],
            observation(2_000)
        ),
        Err(LocalSessionError::CommandConflict { .. })
    ));
}

fn observation(milliseconds: i64) -> TurnObservation {
    TurnObservation::from_unix_milliseconds(milliseconds).expect("valid observation")
}

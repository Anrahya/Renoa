use renoa_agent::ContentBlock;
use renoa_agent_loop::{AgentCommand, ContextContribution, TurnContext};
use renoa_kernel::{AgentId, Command, CommandId, SessionId};
use tempfile::tempdir;

use super::{LocalSession, LocalSessionError, PromptAdmission};

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

#[test]
fn a_retry_gets_the_admitted_command_and_a_new_prompt_gets_the_previous_time() {
    let directory = tempdir().expect("temporary directory");
    let session = session(directory.path());
    let command_id = CommandId::new();
    let content = vec![ContentBlock::text("hello")];
    assert_eq!(
        session
            .prompt_admission(command_id, &content)
            .expect("first"),
        PromptAdmission::New {
            previous_observed_at: None
        }
    );
    let entries = TurnContext::new(vec![
        ContextContribution::plugin("renoa.time", "first").expect("entry"),
    ])
    .expect("context");
    let first = AgentCommand::observed(content.clone(), 1_000, entries).expect("command");
    admit(
        &session,
        command_id,
        &serde_json::to_value(&first).expect("encode"),
    );

    assert_eq!(
        session
            .prompt_admission(command_id, &content)
            .expect("retry"),
        PromptAdmission::Admitted(first)
    );
    assert_eq!(
        session
            .prompt_admission(CommandId::new(), &content)
            .expect("next"),
        PromptAdmission::New {
            previous_observed_at: Some(1_000)
        }
    );
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

    let PromptAdmission::Admitted(replayed) = session
        .prompt_admission(command_id, &[ContentBlock::text("hello")])
        .expect("recover the stored command")
    else {
        panic!("a retry must reuse the stored command");
    };
    assert_eq!(
        serde_json::to_value(&replayed).expect("re-encode"),
        stored,
        "the stored command is reused byte for byte"
    );
    assert_eq!(
        session
            .prompt_admission(CommandId::new(), &[ContentBlock::text("next")])
            .expect("next"),
        PromptAdmission::New {
            previous_observed_at: Some(2_000)
        }
    );
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
        session.prompt_admission(command_id, &[ContentBlock::text("different")]),
        Err(LocalSessionError::CommandConflict { .. })
    ));
}

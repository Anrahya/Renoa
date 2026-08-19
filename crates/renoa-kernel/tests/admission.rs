use renoa_kernel::{AgentId, Command, CommandId, Kernel, KernelError, SessionId};
use tempfile::tempdir;

#[test]
fn agents_and_sessions_are_isolated() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let first_agent = AgentId::new();
    let second_agent = AgentId::new();
    let first_session = SessionId::new();
    let second_session = SessionId::new();

    kernel
        .create_agent(first_agent)
        .expect("create first agent");
    kernel
        .create_agent(second_agent)
        .expect("create second agent");
    kernel
        .create_session(first_session, first_agent)
        .expect("create first session");
    kernel
        .create_session(second_session, second_agent)
        .expect("create second session");

    kernel
        .submit(
            first_session,
            Command::new(CommandId::new(), serde_json::json!({"prompt": "first"})),
        )
        .expect("submit first command");

    let first = kernel
        .inspect(first_session)
        .expect("inspect first session");
    let second = kernel
        .inspect(second_session)
        .expect("inspect second session");
    assert_eq!(first.agent_id, first_agent);
    assert_eq!(first.operations.len(), 1);
    assert_eq!(
        first.operations[0].command.content(),
        &serde_json::json!({"prompt": "first"})
    );
    assert_eq!(second.agent_id, second_agent);
    assert!(second.operations.is_empty());
}

#[test]
fn stable_command_admission_is_exact_and_gapless() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let agent_id = AgentId::new();
    let session_id = SessionId::new();
    let first_id = CommandId::new();
    let first = Command::new(first_id, serde_json::json!({"prompt": "first"}));

    kernel.create_agent(agent_id).expect("create agent");
    kernel
        .create_session(session_id, agent_id)
        .expect("create session");
    let admitted = kernel
        .submit(session_id, first.clone())
        .expect("admit first command");
    let retried = kernel
        .submit(session_id, first)
        .expect("retry exact command");
    assert_eq!(retried, admitted);
    assert_eq!(admitted.position, 0);

    let second = kernel
        .submit(
            session_id,
            Command::new(CommandId::new(), serde_json::json!({"prompt": "second"})),
        )
        .expect("admit second command");
    assert_eq!(second.position, 1);

    let conflict = kernel
        .submit(
            session_id,
            Command::new(first_id, serde_json::json!({"prompt": "changed"})),
        )
        .expect_err("changed command must conflict");
    assert!(matches!(
        conflict,
        KernelError::CommandConflict { command_id, .. } if command_id == first_id
    ));

    let other_session = SessionId::new();
    kernel
        .create_session(other_session, agent_id)
        .expect("create other session");
    assert!(matches!(
        kernel.submit(
            other_session,
            Command::new(first_id, serde_json::json!({"prompt": "first"})),
        ),
        Err(KernelError::CommandConflict { command_id, .. }) if command_id == first_id
    ));
}

#[test]
fn session_identity_cannot_move_between_agents() {
    let directory = tempdir().expect("temporary directory");
    let kernel = Kernel::open(directory.path().join("kernel.sqlite3")).expect("open kernel");
    let first_agent = AgentId::new();
    let second_agent = AgentId::new();
    let session_id = SessionId::new();
    kernel
        .create_agent(first_agent)
        .expect("create first agent");
    kernel
        .create_agent(second_agent)
        .expect("create second agent");
    kernel
        .create_session(session_id, first_agent)
        .expect("create session");
    kernel
        .create_session(session_id, first_agent)
        .expect("retry exact session creation");

    assert!(matches!(
        kernel.create_session(session_id, second_agent),
        Err(KernelError::SessionConflict {
            session_id: found_session,
            agent_id: found_agent,
        }) if found_session == session_id && found_agent == first_agent
    ));
}

use std::path::PathBuf;

use renoa_control::TaskId;
use renoa_protocol::{
    CommandEnvelope, CommandId, CommandInput, PrincipalId, SurfaceRef, TargetRef,
};
use uuid::Uuid;

use super::{NodeStore, TargetBinding};

fn proposal(session: u128) -> TargetBinding {
    TargetBinding {
        target: "agent:alpha".to_owned(),
        agent_id: Uuid::from_u128(1),
        session_id: Uuid::from_u128(session),
        workspace: PathBuf::from("/srv/renoa/alpha"),
    }
}

fn command(id: u128) -> CommandEnvelope {
    CommandEnvelope {
        command_id: CommandId::from_uuid(Uuid::from_u128(id)),
        principal_id: PrincipalId::from_uuid(Uuid::from_u128(2)),
        surface: SurfaceRef::new("discord"),
        target: TargetRef::new("agent:alpha"),
        input: CommandInput::Text {
            text: "continue".to_owned(),
        },
    }
}

#[tokio::test]
async fn a_task_keeps_the_session_recorded_by_its_first_command() {
    let files = tempfile::tempdir().expect("temporary directory");
    let store = NodeStore::open(files.path().join("node.sqlite")).expect("open node ledger");
    let task_id = TaskId::from_uuid(Uuid::from_u128(3));

    let first = store
        .admit(task_id, command(10), proposal(100))
        .await
        .expect("admit the first command");
    let redelivered = store
        .admit(task_id, command(10), proposal(101))
        .await
        .expect("a redelivered command converges on its admission");
    let second = store
        .admit(task_id, command(11), proposal(102))
        .await
        .expect("admit a later command on the same task");

    assert_eq!(first.binding.session_id, Uuid::from_u128(100));
    assert_eq!(redelivered, first);
    assert_eq!(second.binding.session_id, Uuid::from_u128(100));
}

#[tokio::test]
async fn a_task_cannot_move_to_another_agent() {
    let files = tempfile::tempdir().expect("temporary directory");
    let store = NodeStore::open(files.path().join("node.sqlite")).expect("open node ledger");
    let task_id = TaskId::from_uuid(Uuid::from_u128(3));
    store
        .admit(task_id, command(10), proposal(100))
        .await
        .expect("admit the first command");

    let mut moved = proposal(101);
    moved.agent_id = Uuid::from_u128(9);
    assert!(store.admit(task_id, command(11), moved).await.is_err());
}

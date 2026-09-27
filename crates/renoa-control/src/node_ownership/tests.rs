use std::time::{Duration, SystemTime};

use renoa_protocol::PrincipalId;
use rusqlite::params;
use tempfile::TempDir;
use uuid::Uuid;

use super::node_owner;
use crate::{
    NodeId, control_schema::open_connection, coordinator::ControlErrorKind, store::ControlStore,
};

fn principal(value: u128) -> PrincipalId {
    PrincipalId::from_uuid(Uuid::from_u128(value))
}

fn node(value: u128) -> NodeId {
    NodeId::from_uuid(Uuid::from_u128(value))
}

fn enrollments(store: &ControlStore) -> i64 {
    open_connection(&store.path)
        .expect("open control database")
        .query_row("SELECT COUNT(*) FROM enrollments", [], |row| row.get(0))
        .expect("count enrollments")
}

#[tokio::test]
async fn a_node_enrollment_records_its_owner_and_refuses_another() {
    let files = TempDir::new().expect("temporary directory");
    let store = ControlStore::open(files.path().join("control.sqlite")).expect("open store");
    let expires = SystemTime::now() + Duration::from_mins(5);

    store
        .create_node_enrollment(node(1), principal(2), expires)
        .await
        .expect("enroll node for its owner");
    store
        .create_node_enrollment(node(1), principal(2), expires)
        .await
        .expect("re-enroll node for the same owner");
    let refused = store
        .create_node_enrollment(node(1), principal(3), expires)
        .await
        .expect_err("another principal cannot take the node");

    assert_eq!(refused.kind(), ControlErrorKind::Conflict);
    assert_eq!(
        enrollments(&store),
        2,
        "a refused owner leaves no enrollment"
    );
    let connection = open_connection(&store.path).expect("open control database");
    assert_eq!(
        node_owner(&connection, node(1)).expect("read owner"),
        Some(principal(2))
    );
}

#[test]
fn upgrading_adopts_the_single_owner_of_each_node() {
    let files = TempDir::new().expect("temporary directory");
    let path = files.path().join("control.sqlite");
    ControlStore::open(&path).expect("create current schema");
    {
        let connection = open_connection(&path).expect("open control database");
        connection
            .execute_batch("DROP TABLE node_owners; PRAGMA user_version = 11;")
            .expect("rewind to schema 11");
        for (task, owner, node_id) in [(10, 2, 1), (11, 2, 1), (12, 2, 4), (13, 3, 4)] {
            connection
                .execute(
                    "INSERT INTO tasks (task_id, principal_id, node_id, target_json, next_sequence)
                     VALUES (?1, ?2, ?3, '\"workspace:test\"', 0)",
                    params![
                        Uuid::from_u128(task).to_string(),
                        principal(owner).to_string(),
                        node(node_id).to_string(),
                    ],
                )
                .expect("insert schema 11 task");
        }
    }

    ControlStore::open(&path).expect("upgrade to current schema");

    let connection = open_connection(&path).expect("open control database");
    assert_eq!(
        node_owner(&connection, node(1)).expect("read owner"),
        Some(principal(2))
    );
    assert_eq!(
        node_owner(&connection, node(4)).expect("read shared node owner"),
        None,
        "a node shared by several principals has no adopted owner"
    );
}

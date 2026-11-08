//! A share ticket against a real store: it reads as its principal once, for
//! its own share, until it expires.

use std::time::{Duration, SystemTime};

use renoa_protocol::PrincipalId;
use uuid::Uuid;

use super::SHARE_TICKET_LIFETIME;
use crate::{
    control_schema::open_connection,
    identity::{Secret as _, ShareTicket},
    store::ControlStore,
};

fn store() -> (tempfile::TempDir, ControlStore) {
    let directory = tempfile::tempdir().expect("directory");
    let store = ControlStore::open(directory.path().join("control.sqlite3")).expect("store");
    (directory, store)
}

fn rows(store: &ControlStore) -> i64 {
    open_connection(&store.path)
        .expect("connection")
        .query_row("SELECT count(*) FROM share_tickets", [], |row| row.get(0))
        .expect("count")
}

#[tokio::test]
async fn a_ticket_reads_as_its_principal_once_and_only_for_its_share() {
    let (_directory, store) = store();
    let (owner, share, other) = (
        PrincipalId::from_uuid(Uuid::new_v4()),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    let now = SystemTime::now();
    let ticket = store
        .issue_share_ticket(owner, share, now)
        .await
        .expect("issue");
    for _ in 0..2 {
        let found = store.share_ticket_reader(share, &ticket, now).await;
        assert_eq!(
            found.expect("look up"),
            Some(owner),
            "looking does not use it"
        );
    }
    let found = store.share_ticket_reader(other, &ticket, now).await;
    assert_eq!(found.expect("other share"), None);
    assert_eq!(
        store
            .redeem_share_ticket(other, &ticket, now)
            .await
            .expect("other share"),
        None
    );
    assert_eq!(
        store
            .redeem_share_ticket(share, &ticket, now)
            .await
            .expect("redeem"),
        Some(owner)
    );
    assert_eq!(
        store
            .redeem_share_ticket(share, &ticket, now)
            .await
            .expect("again"),
        None,
        "used once"
    );
    let found = store.share_ticket_reader(share, &ticket, now).await;
    assert_eq!(found.expect("look up"), None, "a used ticket is not found");
    let unknown = ShareTicket::generate().expect("ticket");
    assert_eq!(
        store
            .redeem_share_ticket(share, &unknown, now)
            .await
            .expect("unknown"),
        None
    );
    assert_eq!(rows(&store), 0);
}

#[tokio::test]
async fn an_expired_ticket_reads_as_no_one_and_the_next_issue_drops_it() {
    let (_directory, store) = store();
    let (owner, share) = (PrincipalId::from_uuid(Uuid::new_v4()), Uuid::new_v4());
    let issued = SystemTime::now();
    let ticket = store
        .issue_share_ticket(owner, share, issued)
        .await
        .expect("issue");
    let late = issued + SHARE_TICKET_LIFETIME;
    assert_eq!(
        store
            .redeem_share_ticket(share, &ticket, late)
            .await
            .expect("late"),
        None
    );
    assert_eq!(rows(&store), 1, "a refused redemption deletes nothing");
    let just_in_time = issued + SHARE_TICKET_LIFETIME - Duration::from_millis(1);
    store
        .issue_share_ticket(owner, share, late)
        .await
        .expect("issue again");
    assert_eq!(
        rows(&store),
        1,
        "the expired ticket is gone, the new one kept"
    );
    assert_eq!(
        store
            .redeem_share_ticket(share, &ticket, just_in_time)
            .await
            .expect("gone"),
        None
    );
}

#[test]
fn only_a_well_formed_ticket_is_read_from_a_link() {
    assert!(ShareTicket::from_encoded(&"a".repeat(64)).is_some());
    for bad in ["", "abc", &"g".repeat(64), &"a".repeat(65)] {
        assert!(ShareTicket::from_encoded(bad).is_none(), "{bad}");
    }
}

#[test]
fn a_schema_16_database_gains_an_empty_ticket_table() {
    let (directory, store) = store();
    let connection = open_connection(&store.path).expect("connection");
    connection
        .execute_batch("DROP TABLE share_tickets; PRAGMA user_version = 16;")
        .expect("a schema 16 database");
    drop(connection);
    let reopened = ControlStore::open(directory.path().join("control.sqlite3")).expect("reopen");
    assert_eq!(rows(&reopened), 0);
    let version: i64 = open_connection(&reopened.path)
        .expect("connection")
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("version");
    assert_eq!(version, 18);
}

use std::time::Duration;

use super::*;
use crate::host::plugin_setups::{self, Setup};
use renoa_kernel::SessionId;

/// A conversation of `agent`, and its id.
async fn conversation(h: &LocalHost, agent: AgentId) -> SessionId {
    let workspace = h.agent_workspace(agent).await.expect("workspace");
    let id = Uuid::new_v4();
    drop(
        h.ensure_agent_session(agent, &workspace, id)
            .await
            .expect("conversation"),
    );
    SessionId::from_uuid(id)
}

fn now_ms() -> i64 {
    crate::TurnObservation::now()
        .expect("clock")
        .unix_milliseconds()
}

fn saved(database: &Path) -> i64 {
    rusqlite::Connection::open(database)
        .expect("catalog")
        .query_row("SELECT count(*) FROM host_plugin_setups", [], |row| {
            row.get(0)
        })
        .expect("setups")
}

/// Runs the Host's setup waits until nothing is saved, then stops them.
async fn settle_all(h: &LocalHost) {
    let stop = CancellationToken::new();
    let waits = h.run_plugin_setups(stop.clone());
    let emptied = async {
        while saved(&h.config.database) > 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        stop.cancel();
    };
    tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(waits, emptied)
    })
    .await
    .expect("every saved setup settles");
}

/// The follow-ups of `agent` sent back into `session`, by prompt.
fn notes(database: &Path, agent: AgentId, session: SessionId) -> Vec<String> {
    let db = rusqlite::Connection::open(database).expect("catalog");
    let mut statement = db
        .prepare(
            "SELECT a.prompt FROM host_automations a
             JOIN host_automation_follow_ups f ON f.automation_id = a.id
             WHERE a.agent_id = ?1 AND a.origin_session_id = ?2 AND a.enabled = 1",
        )
        .expect("query");
    statement
        .query_map([agent.to_string(), session.to_string()], |row| row.get(0))
        .expect("rows")
        .collect::<Result<_, _>>()
        .expect("prompts")
}

fn setup(agent: AgentId, session: SessionId, operation: &str, expires_at_ms: i64) -> Setup {
    Setup {
        operation_id: operation.to_owned(),
        agent,
        session,
        arguments: json!({"action": "list", "limit": 5}),
        expires_at_ms: Some(expires_at_ms),
        created_at_ms: now_ms(),
    }
}

/// A saved call is replayed, and what came of it goes back into the
/// conversation it came from, once, as the row goes.
#[tokio::test]
async fn a_saved_setup_comes_back_to_its_conversation_once() {
    let (_d, h, parent, _) = fixture().await;
    let session = conversation(&h, parent).await;
    let call = setup(parent, session, "setup-finished", now_ms() + 600_000);
    assert!(plugin_setups::record(&h.config.database, &call).expect("saved"));
    assert!(
        plugin_setups::record(&h.config.database, &call).expect("saving it again changes nothing")
    );
    let mut other = call.clone();
    other.arguments = json!({"action": "list", "limit": 6});
    assert!(
        !plugin_setups::record(&h.config.database, &other).expect("a different call is refused")
    );
    assert_eq!(saved(&h.config.database), 1);

    settle_all(&h).await;

    let notes = notes(&h.config.database, parent, session);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].starts_with("Setup finished: list\n"),
        "{}",
        notes[0]
    );
    assert!(
        notes[0].contains("\"items\""),
        "the call's own result: {}",
        notes[0]
    );
}

/// A call whose link never reached the person is forgotten by the identity
/// its tool call saved it under, and only that call.
#[tokio::test]
async fn a_setup_whose_link_was_not_delivered_is_forgotten() {
    let (_d, h, parent, _) = fixture().await;
    let session = conversation(&h, parent).await;
    let command = renoa_kernel::CommandId::new();
    let undelivered = setup(
        parent,
        session,
        &crate::mcp::oauth_operation_id(session, Some(command), "call-1"),
        now_ms() + 600_000,
    );
    let kept = setup(
        parent,
        session,
        &crate::mcp::oauth_operation_id(session, Some(command), "call-2"),
        now_ms() + 600_000,
    );
    assert!(plugin_setups::record(&h.config.database, &undelivered).expect("saved"));
    assert!(plugin_setups::record(&h.config.database, &kept).expect("saved"));

    h.forget_plugin_setup(session, command, "call-1")
        .await
        .expect("forgotten");
    h.forget_plugin_setup(session, command, "call-1")
        .await
        .expect("forgetting it again changes nothing");

    assert_eq!(saved(&h.config.database), 1);
    let left: String = rusqlite::Connection::open(&h.config.database)
        .expect("catalog")
        .query_row("SELECT operation_id FROM host_plugin_setups", [], |row| {
            row.get(0)
        })
        .expect("the other setup");
    assert_eq!(left, kept.operation_id);
}

/// A link that expired before the person answered comes back saying so.
#[tokio::test]
async fn an_expired_setup_comes_back_saying_so() {
    let (_d, h, parent, _) = fixture().await;
    let session = conversation(&h, parent).await;
    let call = setup(parent, session, "setup-expired", now_ms() - 600_000);
    assert!(plugin_setups::record(&h.config.database, &call).expect("saved"));

    settle_all(&h).await;

    let notes = notes(&h.config.database, parent, session);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("before it expired"), "{}", notes[0]);
}

/// Stopping the Host keeps what it was waiting on, for the next run, even
/// when the call it was replaying finished as it stopped.
#[tokio::test]
async fn a_stopped_host_keeps_its_saved_setups() {
    let (_d, h, parent, _) = fixture().await;
    let session = conversation(&h, parent).await;
    let call = setup(parent, session, "setup-kept", now_ms() + 600_000);
    assert!(plugin_setups::record(&h.config.database, &call).expect("saved"));
    let stop = CancellationToken::new();
    stop.cancel();
    h.run_plugin_setups(stop.clone()).await;
    plugin_setups::finish(std::sync::Arc::clone(&h.config), call, stop).await;
    assert_eq!(saved(&h.config.database), 1);
    assert!(notes(&h.config.database, parent, session).is_empty());
}

/// A schema 51 catalog gains the table of saved setups when the Host opens it.
#[tokio::test]
async fn schema_52_lets_a_schema_51_host_save_setups() {
    let (d, h, parent, _) = fixture().await;
    let session = conversation(&h, parent).await;
    let connection = rusqlite::Connection::open(&h.config.database).expect("catalog");
    plugin_setups::restore_schema_51(&connection);
    connection
        .execute_batch(
            "UPDATE host_metadata SET schema_version = 51 WHERE singleton = 1;
             PRAGMA user_version = 51;",
        )
        .expect("store schema 51");
    drop(connection);
    drop(h);
    let h = host(d.path());
    let call = setup(parent, session, "setup-migrated", now_ms() + 600_000);
    assert!(plugin_setups::record(&h.config.database, &call).expect("saved after the upgrade"));
}

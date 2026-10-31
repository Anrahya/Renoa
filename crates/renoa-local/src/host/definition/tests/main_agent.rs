use renoa_kernel::AgentId;
use rusqlite::Connection;

use super::*;
use crate::{
    AgentDefinition, HostMainAgentControl, HostObserver, MainAgent, MainAgentChange,
    MainAgentError, host::catalog,
};

async fn make(
    host: &LocalHost,
    (creator, origin): (AgentCreator, AgentCreationOrigin),
    request: AgentCreateRequest,
) -> AgentDefinition {
    host.create_agent(creator, origin, request, CancellationToken::new())
        .await
        .expect("create")
}

async fn owner_creator(host: &LocalHost, owner: Uuid) -> (AgentCreator, AgentCreationOrigin) {
    (
        AgentCreator::Principal {
            host_id: host.host_id().await.expect("Host identity"),
            principal_id: owner.to_string(),
        },
        AgentCreationOrigin::Management,
    )
}

async fn observed(root: &Path) -> Option<MainAgent> {
    HostObserver::open(&root.join("data"))
        .expect("observer")
        .snapshot()
        .await
        .expect("snapshot")
        .main_agent
}

fn main_agent(host: &LocalHost) -> Option<MainAgent> {
    let connection = Connection::open(&host.config.database).expect("catalog");
    super::super::main_agent::read(&connection).expect("marker")
}

fn uuid(agent: AgentId) -> Uuid {
    Uuid::parse_str(&agent.to_string()).expect("an agent id is a UUID")
}

fn marked(agent: AgentId, revision: i64) -> MainAgent {
    MainAgent {
        main_agent_id: uuid(agent),
        revision,
    }
}

#[tokio::test]
async fn the_first_agent_made_is_main_whoever_made_it_and_later_ones_and_replays_leave_it() {
    let (directory, host) = fixture();
    let owner = Uuid::new_v4();
    assert_eq!(observed(directory.path()).await, None, "no agent, no main");
    let first_request = specialist(Uuid::new_v4(), "First");
    let first = make(
        &host,
        owner_creator(&host, owner).await,
        first_request.clone(),
    )
    .await;
    assert_eq!(observed(directory.path()).await, Some(marked(first.id, 1)));

    let provisioned = make(&host, system("test"), specialist(Uuid::new_v4(), "Later")).await;
    // An agent can only be made by an agent that already exists, so it is
    // never the first; it must not take the mark either.
    make(
        &host,
        (
            AgentCreator::Agent { agent_id: first.id },
            AgentCreationOrigin::AgentTool,
        ),
        specialist(Uuid::new_v4(), "Child"),
    )
    .await;
    assert_eq!(observed(directory.path()).await, Some(marked(first.id, 1)));

    let control = HostMainAgentControl::open(
        &directory.path().join("data"),
        host.host_id().await.expect("Host identity"),
        owner,
    )
    .expect("control");
    control
        .set(
            owner,
            MainAgentChange {
                operation_id: Uuid::new_v4(),
                expected_revision: 1,
                agent_id: uuid(provisioned.id),
            },
        )
        .await
        .expect("name another main agent");
    let replayed = make(&host, owner_creator(&host, owner).await, first_request).await;
    assert_eq!(replayed.id, first.id, "the creation replays its receipt");
    assert_eq!(
        observed(directory.path()).await,
        Some(marked(provisioned.id, 2)),
        "a replayed creation leaves the owner's choice"
    );

    let (_other, provisioned_first) = fixture();
    let first = make(
        &provisioned_first,
        system("test"),
        specialist(Uuid::new_v4(), "Provisioned"),
    )
    .await;
    assert_eq!(main_agent(&provisioned_first), Some(marked(first.id, 1)));
}

#[tokio::test]
async fn the_observation_names_the_main_agent_and_its_revision() {
    let (directory, host) = fixture();
    let first = make(&host, system("test"), specialist(Uuid::new_v4(), "First")).await;
    let snapshot = HostObserver::open(&directory.path().join("data"))
        .expect("observer")
        .snapshot()
        .await
        .expect("snapshot");
    let json = serde_json::to_value(&snapshot).expect("observation JSON");
    assert_eq!(
        json["main_agent"],
        serde_json::json!({"main_agent_id": uuid(first.id), "revision": 1})
    );
}

/// Stores the catalog in its schema 41 shape, with each agent created at the
/// given time, as an earlier runtime left it.
fn downgrade(host: &LocalHost, created: &[(AgentId, i64)]) {
    let connection = Connection::open(&host.config.database).expect("catalog");
    catalog::restore_schema_41(&connection);
    for (agent, at) in created {
        connection
            .execute(
                "UPDATE host_agents SET created_at_ms = ?2 WHERE agent_id = ?1",
                rusqlite::params![agent.to_string(), at],
            )
            .expect("creation time");
    }
    connection
        .execute_batch(
            "UPDATE host_metadata SET schema_version = 41 WHERE singleton = 1;
             PRAGMA user_version = 41;",
        )
        .expect("schema 41");
}

fn upgrade_twice(host: &LocalHost) {
    catalog::initialize(&host.config.database).expect("upgrade schema 41");
    catalog::initialize(&host.config.database).expect("reopen the upgraded catalog");
}

/// Two agents, the one whose id sorts first first.
async fn two_agents_by_id(host: &LocalHost) -> [AgentId; 2] {
    let one = make(host, system("test"), specialist(Uuid::new_v4(), "One"))
        .await
        .id;
    let two = make(host, system("test"), specialist(Uuid::new_v4(), "Two"))
        .await
        .id;
    if uuid(one) < uuid(two) {
        [one, two]
    } else {
        [two, one]
    }
}

#[tokio::test]
async fn schema_42_marks_the_earliest_made_agent_whichever_way_the_ids_sort() {
    for earliest_sorts_first in [true, false] {
        let (_directory, host) = fixture();
        let ids = two_agents_by_id(&host).await;
        let (earliest, latest) = if earliest_sorts_first {
            (ids[0], ids[1])
        } else {
            (ids[1], ids[0])
        };
        downgrade(&host, &[(earliest, 1_000), (latest, 2_000)]);
        upgrade_twice(&host);
        assert_eq!(
            main_agent(&host),
            Some(marked(earliest, 1)),
            "earliest sorts first: {earliest_sorts_first}"
        );
    }

    let (_directory, host) = fixture();
    let ids = two_agents_by_id(&host).await;
    downgrade(&host, &[(ids[0], 1_000), (ids[1], 1_000)]);
    upgrade_twice(&host);
    assert_eq!(
        main_agent(&host),
        Some(marked(ids[0], 1)),
        "a tie goes by id"
    );
}

#[tokio::test]
async fn schema_42_leaves_an_empty_host_unmarked_until_its_first_agent() {
    let (_directory, host) = fixture();
    downgrade(&host, &[]);
    upgrade_twice(&host);
    assert_eq!(main_agent(&host), None);
    let first = make(&host, system("test"), specialist(Uuid::new_v4(), "First")).await;
    assert_eq!(main_agent(&host), Some(marked(first.id, 1)));
}

#[tokio::test]
async fn only_the_owner_changes_the_main_agent_from_the_revision_it_read() {
    let (directory, host) = fixture();
    let owner = Uuid::new_v4();
    let first = make(&host, system("test"), specialist(Uuid::new_v4(), "First")).await;
    let second = make(&host, system("test"), specialist(Uuid::new_v4(), "Second")).await;
    let control = HostMainAgentControl::open(
        &directory.path().join("data"),
        host.host_id().await.expect("Host identity"),
        owner,
    )
    .expect("control");
    let change = |expected_revision, agent: Uuid| MainAgentChange {
        operation_id: Uuid::new_v4(),
        expected_revision,
        agent_id: agent,
    };
    let to_second = change(1, uuid(second.id));

    assert!(matches!(
        control.set(Uuid::new_v4(), to_second).await,
        Err(MainAgentError::Forbidden)
    ));
    assert!(matches!(
        control.set(owner, change(2, uuid(second.id))).await,
        Err(MainAgentError::Stale)
    ));
    let unknown = Uuid::new_v4();
    assert!(matches!(
        control.set(owner, change(1, unknown)).await,
        Err(MainAgentError::AgentNotFound(id)) if id == unknown
    ));
    assert_eq!(observed(directory.path()).await, Some(marked(first.id, 1)));

    let changed = control.set(owner, to_second).await.expect("change");
    assert_eq!(changed, marked(second.id, 2));
    assert_eq!(observed(directory.path()).await, Some(marked(second.id, 2)));
    control
        .set(owner, change(2, uuid(first.id)))
        .await
        .expect("change back");

    assert_eq!(
        control.set(owner, to_second).await.expect("retry"),
        changed,
        "an identical retry returns its first answer, even after a later change"
    );
    let reused = MainAgentChange {
        agent_id: uuid(first.id),
        ..to_second
    };
    assert!(matches!(
        control.set(owner, reused).await,
        Err(MainAgentError::OperationReused)
    ));
    assert_eq!(observed(directory.path()).await, Some(marked(first.id, 3)));
    let receipts: i64 = Connection::open(&host.config.database)
        .expect("catalog")
        .query_row(
            "SELECT count(*) FROM host_main_agent_operations",
            [],
            |row| row.get(0),
        )
        .expect("receipts");
    assert_eq!(receipts, 2, "a refusal leaves no receipt");
}

/// Deletes every row naming `agent` except the marker's, then the agent itself.
fn delete_agent(connection: &Connection, agent: AgentId) -> rusqlite::Result<usize> {
    let mut children = Vec::new();
    let mut tables = connection.prepare(
        "SELECT m.name, f.\"from\" FROM sqlite_master AS m, pragma_foreign_key_list(m.name) AS f
         WHERE m.type = 'table' AND f.\"table\" = 'host_agents'
           AND m.name NOT IN ('host_agents', 'host_main_agent')",
    )?;
    let mut rows = tables.query([])?;
    while let Some(row) = rows.next()? {
        children.push((row.get::<_, String>(0)?, row.get::<_, String>(1)?));
    }
    for (table, column) in children {
        connection.execute(
            &format!("DELETE FROM {table} WHERE {column} = ?1"),
            [agent.to_string()],
        )?;
    }
    connection.execute(
        "DELETE FROM host_agents WHERE agent_id = ?1",
        [agent.to_string()],
    )
}

#[tokio::test]
async fn the_main_agent_cannot_be_deleted_and_a_reset_clears_the_mark() {
    let (directory, host) = fixture();
    let first = make(&host, system("test"), specialist(Uuid::new_v4(), "First")).await;
    let second = make(&host, system("test"), specialist(Uuid::new_v4(), "Second")).await;
    let connection = Connection::open(&host.config.database).expect("catalog");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enforce foreign keys");
    assert_eq!(
        delete_agent(&connection, second.id).expect("another agent can go"),
        1
    );
    let refused = delete_agent(&connection, first.id).expect_err("the main agent stays");
    assert!(
        refused
            .to_string()
            .contains("FOREIGN KEY constraint failed"),
        "{refused}"
    );
    assert_eq!(main_agent(&host), Some(marked(first.id, 1)));
    drop(connection);
    drop(host);

    crate::reset_host_data_root(&directory.path().join("data")).expect("reset");
    let host = super::host(directory.path());
    assert_eq!(main_agent(&host), None, "a reset clears the mark");
    let fresh = make(&host, system("test"), specialist(Uuid::new_v4(), "Fresh")).await;
    assert_eq!(main_agent(&host), Some(marked(fresh.id, 1)));
}

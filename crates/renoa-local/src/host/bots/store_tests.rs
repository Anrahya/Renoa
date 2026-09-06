use super::*;
use crate::BotRecipe;

fn fixture() -> (tempfile::TempDir, BotRecord) {
    let directory = tempfile::tempdir().expect("fixture");
    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("catalog");
    let parent = AgentId::new();
    catalog::open_verified(&database)
        .expect("catalog")
        .execute(
            "INSERT INTO host_agents VALUES (?1,'fixture','Parent',NULL)",
            [parent.to_string()],
        )
        .expect("parent");
    (
        directory,
        BotRecord {
            id: AgentId::new(),
            created_by: parent,
            recipe: BotRecipe {
                name: "News".to_owned(),
                instructions: "Read only.".to_owned(),
                tools: std::collections::BTreeSet::new(),
                connections: std::collections::BTreeSet::new(),
            },
        },
    )
}

#[test]
fn incomplete_catalog_rejects_creation_without_leaving_agent_or_profile_bindings() {
    let (directory, mut record) = fixture();
    let path = directory.path().join("host.sqlite3");
    let connection = catalog::open_verified(&path).expect("catalog");
    connection.execute_batch("INSERT INTO mcp_integrations(integration_id,kind,endpoint,request_headers_json) VALUES ('test','direct_streamable_http','https://example.com/mcp','{}');
        INSERT INTO mcp_connections(connection_id,integration_id,auth_kind) VALUES ('undiscovered','test','none');").expect("registered but not discovered");
    record.recipe.connections.insert("undiscovered".to_owned());
    assert!(
        matches!(ensure(&path,record.clone(),&CancellationToken::new()),Err(LocalHostError::InvalidRequest(message)) if message.contains("no complete catalog"))
    );
    assert!(get(&path, record.id).expect("lookup").is_none());
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM host_agents WHERE agent_id=?1",
                [record.id.to_string()],
                |row| row.get::<_, i64>(0)
            )
            .expect("agents"),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM profile_mcp_connections", [], |row| {
                row.get::<_, i64>(0)
            })
            .expect("bindings"),
        0
    );
}

#[test]
fn cancellation_observed_after_acquiring_the_write_transaction_prevents_publication() {
    let (directory, record) = fixture();
    let path = directory.path().join("host.sqlite3");
    let mut connection = catalog::open_verified(&path).expect("catalog");
    let cancellation = CancellationToken::new();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .expect("write boundary");
    cancellation.cancel();
    assert!(matches!(
        publish(transaction, record.clone(), &cancellation),
        Err(LocalHostError::BotCreationCancelled)
    ));
    assert!(get(&path, record.id).expect("lookup").is_none());
}

#[test]
fn inventory_uses_bounded_compact_pages_without_repeating_or_omitting_bots() {
    let (directory, mut record) = fixture();
    let path = directory.path().join("host.sqlite3");
    let mut expected = std::collections::BTreeSet::new();
    for _ in 0..25 {
        record.id = AgentId::new();
        record.recipe.instructions = "long-instructions".repeat(1000);
        ensure(&path, record.clone(), &CancellationToken::new()).expect("create");
        expected.insert(record.id.to_string());
    }
    let first = list(&path, None).expect("first page");
    assert_eq!(first.bots.len(), 20);
    assert!(first.next_cursor.is_some());
    assert!(
        !serde_json::to_string(&first)
            .expect("compact page")
            .contains("long-instructions")
    );
    let second = list(&path, first.next_cursor).expect("second page");
    assert_eq!(second.bots.len(), 5);
    assert!(second.next_cursor.is_none());
    assert_eq!(
        first
            .bots
            .into_iter()
            .chain(second.bots)
            .map(|bot| bot.id.to_string())
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
}

#[test]
fn schema_fourteen_upgrade_preserves_existing_agent_identity() {
    let (directory, record) = fixture();
    let path = directory.path().join("host.sqlite3");
    catalog::open_verified(&path).expect("catalog").execute_batch("DROP TABLE host_bots; UPDATE host_metadata SET schema_version=14; PRAGMA user_version=14;").expect("old schema");
    catalog::initialize(&path).expect("upgrade");
    ensure(&path, record.clone(), &CancellationToken::new())
        .expect("existing creator still present");
    assert_eq!(get(&path, record.id).expect("lookup"), Some(record));
}

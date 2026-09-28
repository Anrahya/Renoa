use super::{HostCatalogError, cutover, initialize, open_verified};

#[test]
fn schema_29_removes_retired_mcp_loader_from_stored_selection() {
    let directory = tempfile::tempdir().expect("temporary Host catalog");
    let database = directory.path().join("host.sqlite3");
    initialize(&database).expect("initialize current catalog");
    {
        let connection = open_verified(&database).expect("open current catalog");
        connection
            .execute_batch(
                "INSERT INTO host_agents(agent_id, name, created_at_ms, created_via,
                preset_id, operational_json, creator_kind, creator_component)
             VALUES ('00000000-0000-0000-0000-000000000001', 'Fixture', 1,
                'provisioning', NULL, '{}', 'system', 'migration-test');
             INSERT INTO host_agent_tool_selections(agent_id, revision, tools_json)
             VALUES ('00000000-0000-0000-0000-000000000001', 1,
                '[\"plugin_search\",\"tool_load\",\"tool_execute\"]');
             UPDATE host_metadata SET schema_version = 29 WHERE singleton = 1;
             PRAGMA user_version = 29;",
            )
            .expect("construct schema-29 selection");
    }
    initialize(&database).expect("migrate schema-29 selection");
    let connection = open_verified(&database).expect("open migrated catalog");
    let (revision, tools): (i64, String) = connection
        .query_row(
            "SELECT revision, tools_json FROM host_agent_tool_selections WHERE agent_id = ?1",
            ["00000000-0000-0000-0000-000000000001"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read migrated selection");
    assert_eq!(revision, 2);
    assert_eq!(tools, "[]");
}

#[test]
fn schema_28_selections_remove_retired_host_tool_names_once() {
    let directory = tempfile::tempdir().expect("temporary Host catalog");
    let database = directory.path().join("host.sqlite3");
    initialize(&database).expect("initialize current catalog");
    {
        let connection = open_verified(&database).expect("open current catalog");
        connection
            .execute_batch(
                "INSERT INTO host_agents(agent_id, name, created_at_ms, created_via,
                preset_id, operational_json, creator_kind, creator_component)
             VALUES ('00000000-0000-0000-0000-000000000001', 'Fixture', 1,
                'provisioning', NULL, '{}', 'system', 'migration-test');
             INSERT INTO host_agent_tool_selections(agent_id, revision, tools_json)
             VALUES ('00000000-0000-0000-0000-000000000001', 4,
                '[\"extension_manage\",\"tool_search\",\"tool_load\",\"bash\"]');
             INSERT INTO host_agent_creations(operation_id, agent_id, request_json, result_json)
             VALUES ('00000000-0000-0000-0000-000000000002',
                '00000000-0000-0000-0000-000000000001', '{}',
                '{\"tool_selection\":{\"revision\":1,\"tools\":[\"extension_manage\",\"tool_search\",\"tool_load\"]}}');
             INSERT INTO host_agent_renames(operation_id, agent_id, actor_agent_id, request_json, result_json)
             VALUES ('00000000-0000-0000-0000-000000000003',
                '00000000-0000-0000-0000-000000000001',
                '00000000-0000-0000-0000-000000000001', '{}',
                '{\"tool_selection\":{\"revision\":2,\"tools\":[\"extension_manage\",\"tool_load\"]}}');
             INSERT INTO host_agent_tool_selection_operations(operation_id, request_json, result_json)
             VALUES ('00000000-0000-0000-0000-000000000004', '{}',
                '{\"revision\":3,\"tools\":[\"tool_search\",\"tool_load\"]}');
             UPDATE host_metadata SET schema_version = 28 WHERE singleton = 1;
             PRAGMA user_version = 28;",
            )
            .expect("construct schema-28 selection");
    }
    initialize(&database).expect("migrate schema-28 selections");
    initialize(&database).expect("reopening the current catalog is stable");
    let connection = open_verified(&database).expect("open migrated catalog");
    let (revision, encoded): (i64, String) = connection
        .query_row(
            "SELECT revision, tools_json FROM host_agent_tool_selections WHERE agent_id = ?1",
            ["00000000-0000-0000-0000-000000000001"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("read migrated selection");
    assert_eq!(revision, 5);
    assert_eq!(encoded, r#"["bash"]"#);
    for (table, expected) in [
        ("host_agent_creations", "[]"),
        ("host_agent_renames", "[]"),
        ("host_agent_tool_selection_operations", "[]"),
    ] {
        let path = if table == "host_agent_tool_selection_operations" {
            "$.tools"
        } else {
            "$.tool_selection.tools"
        };
        let selected: String = connection
            .query_row(
                &format!("SELECT json_extract(result_json, '{path}') FROM {table}"),
                [],
                |row| row.get(0),
            )
            .expect("read migrated receipt");
        assert_eq!(
            selected, expected,
            "{table} receipt must contain only machine grants"
        );
    }
}

#[test]
fn schema_ten_adds_an_unbound_shared_registry_cursor() {
    let directory = tempfile::tempdir().expect("temporary Host catalog");
    let database = directory.path().join("host.sqlite3");
    initialize(&database).expect("initialize current catalog");
    {
        let connection = open_verified(&database).expect("open current catalog");
        connection
            .execute_batch(
                "INSERT INTO mcp_integrations(
                    integration_id, kind, endpoint, request_headers_json
                 ) VALUES (
                    'retained', 'direct_streamable_http', 'https://example.com/mcp', '{}'
                 );
                 DROP TABLE host_agents;
                 DROP TABLE host_identity;
                 DROP TABLE shared_plugin_registry_state;
                 UPDATE host_metadata SET schema_version = 10 WHERE singleton = 1;
                 PRAGMA user_version = 10;",
            )
            .expect("construct schema-ten fixture");
    }
    let refused = initialize(&database);
    assert!(
        matches!(&refused, Err(HostCatalogError::Invalid(message)) if message.contains("reset")),
        "an earlier data root must be refused until it is reset: {refused:?}"
    );
    cutover(&database).expect("cut over schema ten");
    initialize(&database).expect("open after the cutover");
    let connection = open_verified(&database).expect("open cut-over catalog");
    let rows = connection
        .query_row(
            "SELECT COUNT(*) FROM shared_plugin_registry_state",
            [],
            |row| row.get::<_, u32>(0),
        )
        .expect("read shared registry state");
    assert_eq!(rows, 0);
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM mcp_integrations WHERE integration_id = 'retained'",
                [],
                |row| row.get::<_, u32>(0),
            )
            .expect("retained integration"),
        1
    );
}

#[test]
fn a_creation_receipt_without_a_result_is_refused_and_repaired() {
    let directory = tempfile::tempdir().expect("temporary Host catalog");
    let database = directory.path().join("host.sqlite3");
    initialize(&database).expect("initialize current catalog");
    {
        let connection = open_verified(&database).expect("open current catalog");
        connection
            .execute_batch(
                "DROP TABLE host_agent_creations;
                 CREATE TABLE host_agent_creations (
                    operation_id TEXT PRIMARY KEY,
                    agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
                    request_json TEXT NOT NULL CHECK (json_valid(request_json))
                 ) STRICT;
                 UPDATE host_metadata SET schema_version = 27 WHERE singleton = 1;
                 PRAGMA user_version = 27;",
            )
            .expect("construct schema-twenty-seven fixture");
    }
    let refused = initialize(&database);
    assert!(
        matches!(&refused, Err(HostCatalogError::Invalid(message)) if message.contains("reset")),
        "a receipt without a stored result must be refused until it is reset: {refused:?}"
    );
    cutover(&database).expect("cut over schema twenty-seven");
    initialize(&database).expect("open after the cutover");
    let connection = open_verified(&database).expect("open cut-over catalog");
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('host_agent_creations')
                 WHERE name = 'result_json'",
                [],
                |row| row.get::<_, u32>(0),
            )
            .expect("read creation receipt columns"),
        1,
        "the cutover must restore the canonical receipt shape"
    );
}

/// Schema 32 carried the GitHub review tables. Their rows reference each other
/// and an agent while foreign keys are enforced, so children must drop first.
#[test]
fn schema_32_drops_the_retired_review_tables_in_place() {
    let directory = tempfile::tempdir().expect("temporary Host catalog");
    let database = directory.path().join("host.sqlite3");
    initialize(&database).expect("initialize current catalog");
    {
        let connection = open_verified(&database).expect("open current catalog");
        connection
            .execute_batch(
                "INSERT INTO host_agents(agent_id, name, created_at_ms, created_via,
                    preset_id, operational_json, creator_kind, creator_component)
                 VALUES ('00000000-0000-0000-0000-000000000001', 'Reviewer', 1,
                    'provisioning', NULL, '{}', 'system', 'migration-test');
                 CREATE TABLE host_review_repositories (
                    repository_id INTEGER PRIMARY KEY CHECK(repository_id>0),
                    agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
                    record_json TEXT NOT NULL CHECK(json_valid(record_json))
                 ) STRICT;
                 CREATE TABLE host_review_operations (
                    operation_id TEXT PRIMARY KEY, request_json TEXT NOT NULL,
                    result_json TEXT NOT NULL
                 ) STRICT;
                 CREATE TABLE host_review_requests (
                    sequence INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
                    repository_id INTEGER NOT NULL
                        REFERENCES host_review_repositories(repository_id)
                 ) STRICT;
                 CREATE TABLE host_review_deliveries (
                    delivery_id TEXT PRIMARY KEY, result_json TEXT NOT NULL
                 ) STRICT;
                 CREATE TABLE host_review_runs (
                    request_id TEXT PRIMARY KEY REFERENCES host_review_requests(id)
                 ) STRICT;
                 CREATE TABLE host_review_jobs (
                    request_id TEXT PRIMARY KEY REFERENCES host_review_requests(id)
                 ) STRICT;
                 CREATE TABLE host_review_publications (
                    request_id TEXT PRIMARY KEY REFERENCES host_review_requests(id)
                 ) STRICT;
                 INSERT INTO host_review_repositories
                 VALUES (7, '00000000-0000-0000-0000-000000000001', '{}');
                 INSERT INTO host_review_operations VALUES ('operation', '{}', '{}');
                 INSERT INTO host_review_requests(id, repository_id) VALUES ('request', 7);
                 INSERT INTO host_review_deliveries VALUES ('delivery', '{}');
                 INSERT INTO host_review_runs VALUES ('request');
                 INSERT INTO host_review_jobs VALUES ('request');
                 INSERT INTO host_review_publications VALUES ('request');
                 UPDATE host_metadata SET schema_version = 32 WHERE singleton = 1;
                 PRAGMA user_version = 32;",
            )
            .expect("construct schema-32 review tables");
    }
    initialize(&database).expect("upgrade schema 32 in place");
    initialize(&database).expect("reopening the upgraded catalog is stable");
    let connection = open_verified(&database).expect("open upgraded catalog");
    let review_tables: u32 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'host\\_review\\_%' ESCAPE '\\'",
            [],
            |row| row.get(0),
        )
        .expect("count review tables");
    assert_eq!(review_tables, 0, "schema 33 must retire every review table");
    let agents: u32 = connection
        .query_row("SELECT COUNT(*) FROM host_agents", [], |row| row.get(0))
        .expect("count agents");
    assert_eq!(agents, 1, "an in-place upgrade keeps agent rows");
}

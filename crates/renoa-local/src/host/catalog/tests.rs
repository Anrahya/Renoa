use super::{
    HostCatalogError, cutover, initialize, open_verified, restore_schema_34_automations,
    restore_schema_35, restore_schema_36, restore_schema_37,
};

#[test]
fn schema_29_removes_retired_mcp_loader_from_stored_selection() {
    let directory = tempfile::tempdir().expect("temporary Host catalog");
    let database = directory.path().join("host.sqlite3");
    initialize(&database).expect("initialize current catalog");
    {
        let connection = open_verified(&database).expect("open current catalog");
        super::restore_routine_tables(&connection);
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
        super::restore_routine_tables(&connection);
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
        super::restore_routine_tables(&connection);
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
        super::restore_routine_tables(&connection);
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

/// Builds a schema 33 catalog holding one automation with a pending run, a
/// deletion, both receipt kinds, and a disabled `renoa.routines` activation.
fn schema_33_with_routines(database: &std::path::Path) {
    initialize(database).expect("initialize current catalog");
    let connection = open_verified(database).expect("open current catalog");
    connection
        .execute_batch(
            "INSERT INTO host_agents(agent_id, name, created_at_ms, created_via,
                preset_id, operational_json, creator_kind, creator_component)
             VALUES ('00000000-0000-0000-0000-000000000001', 'Scheduler', 1,
                'provisioning', NULL, '{}', 'system', 'migration-test');
             INSERT INTO host_automations(id, agent_id, name, prompt, schedule_json,
                enabled, revision, next_due_ms)
             VALUES ('00000000-0000-0000-0000-00000000000a',
                '00000000-0000-0000-0000-000000000001', 'Morning', 'Summarize.',
                '{\"interval\":{\"hours\":24}}', 1, 2, 100);
             INSERT INTO host_automation_runs(id, automation_id, agent_id,
                due_ms, admitted_at_ms, submission, output)
             VALUES ('00000000-0000-0000-0000-00000000000b',
                '00000000-0000-0000-0000-00000000000a',
                '00000000-0000-0000-0000-000000000001', 100, 101, 'Summarize.', NULL);
             INSERT INTO host_automation_deletions VALUES ('00000000-0000-0000-0000-00000000000a');
             INSERT INTO host_automation_mutations VALUES ('00000000-0000-0000-0000-00000000000c',
                '00000000-0000-0000-0000-000000000001', '{}', '{}');
             INSERT INTO host_automation_owner_mutations VALUES ('00000000-0000-0000-0000-00000000000d',
                'owner', '{}', '{}');
             INSERT INTO host_agent_builtin_plugins
             VALUES ('00000000-0000-0000-0000-000000000001', 'renoa.automations', 0);
             INSERT INTO host_builtin_plugin_operations
             VALUES ('00000000-0000-0000-0000-000000000001', 'operation',
                '{\"plugin_id\":\"renoa.automations\",\"enabled\":false}',
                '{\"plugin_id\":\"renoa.automations\",\"enabled\":false}');",
        )
        .expect("seed automation rows");
    super::restore_routine_tables(&connection);
    connection
        .execute_batch(
            "UPDATE host_metadata SET schema_version = 33 WHERE singleton = 1;
             PRAGMA user_version = 33;",
        )
        .expect("mark schema 33");
}

fn count(connection: &rusqlite::Connection, sql: &str) -> u32 {
    connection
        .query_row(sql, [], |row| row.get(0))
        .expect("count rows")
}

#[test]
fn schema_33_renames_routines_to_automations_in_place() {
    let directory = tempfile::tempdir().expect("temporary Host catalog");
    let database = directory.path().join("host.sqlite3");
    schema_33_with_routines(&database);

    initialize(&database).expect("upgrade schema 33 in place");
    initialize(&database).expect("reopening the upgraded catalog is stable");
    let connection = open_verified(&database).expect("open upgraded catalog");
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE '%routine%'"
        ),
        0,
        "no table or index keeps the routine name"
    );
    for (table, rows) in [
        ("host_automations", 1),
        ("host_automation_runs WHERE output IS NULL", 1),
        ("host_automation_deletions", 1),
        ("host_automation_mutations", 1),
        ("host_automation_owner_mutations", 1),
    ] {
        assert_eq!(
            count(&connection, &format!("SELECT COUNT(*) FROM {table}")),
            rows,
            "{table} keeps its rows"
        );
    }
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM host_automation_runs AS run
             JOIN host_automation_deletions AS deletion ON deletion.automation_id = run.automation_id"
        ),
        1,
        "renamed columns still link a run and a deletion to their automation"
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'host_automation_pending'"
        ),
        1
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM host_agent_builtin_plugins
             WHERE plugin_id = 'renoa.automations' AND enabled = 0"
        ),
        1,
        "a disabled plugin stays disabled under its new id"
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM host_builtin_plugin_operations
             WHERE request_json LIKE '%renoa.automations%' AND result_json LIKE '%renoa.automations%'"
        ),
        1
    );
    connection
        .execute(
            "INSERT INTO host_agent_builtin_plugins
             VALUES ('00000000-0000-0000-0000-000000000001', 'renoa.routines', 1)",
            [],
        )
        .expect_err("the old plugin id is no longer allowed");
}

#[test]
fn a_reset_from_schema_33_recreates_the_automation_tables() {
    let directory = tempfile::tempdir().expect("temporary Host catalog");
    let database = directory.path().join("host.sqlite3");
    schema_33_with_routines(&database);

    cutover(&database).expect("reset schema 33");
    let connection = open_verified(&database).expect("open reset catalog");
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE '%routine%'"
        ),
        0
    );
    connection
        .execute_batch(
            "INSERT INTO host_agents(agent_id, name, created_at_ms, created_via,
                preset_id, operational_json, creator_kind, creator_component)
             VALUES ('00000000-0000-0000-0000-000000000002', 'Fresh', 1,
                'provisioning', NULL, '{}', 'system', 'migration-test');
             INSERT INTO host_agent_builtin_plugins
             VALUES ('00000000-0000-0000-0000-000000000002', 'renoa.automations', 0);",
        )
        .expect("a reset catalog accepts the automations plugin id");
}

/// Builds a schema 34 catalog holding one automation with a pending run and a
/// finished one, whose runs still record a private Host session.
fn schema_34_with_runs(database: &std::path::Path) {
    initialize(database).expect("initialize current catalog");
    let connection = open_verified(database).expect("open current catalog");
    restore_schema_34_automations(&connection);
    connection
        .execute_batch(
            "INSERT INTO host_agents(agent_id, name, created_at_ms, created_via,
                preset_id, operational_json, creator_kind, creator_component)
             VALUES ('00000000-0000-0000-0000-000000000001', 'Scheduler', 1,
                'provisioning', NULL, '{}', 'system', 'migration-test');
             INSERT INTO host_automations VALUES ('00000000-0000-0000-0000-00000000000a',
                '00000000-0000-0000-0000-000000000001', 'Morning', 'Summarize.',
                '{\"interval\":{\"hours\":24}}', 1, 2, 100);
             INSERT INTO host_automation_runs(id, automation_id, agent_id, session_id,
                due_ms, admitted_at_ms, prompt, output)
             VALUES ('00000000-0000-0000-0000-00000000000b',
                '00000000-0000-0000-0000-00000000000a',
                '00000000-0000-0000-0000-000000000001',
                '00000000-0000-0000-0000-00000000000e', 100, 101, 'Summarize.', 'Done.'),
                ('00000000-0000-0000-0000-00000000000c',
                '00000000-0000-0000-0000-00000000000a',
                '00000000-0000-0000-0000-000000000001',
                '00000000-0000-0000-0000-00000000000e', 200, 201, 'Summarize.', NULL);
             UPDATE host_metadata SET schema_version = 34 WHERE singleton = 1;
             PRAGMA user_version = 34;",
        )
        .expect("seed schema 34 automation rows");
}

#[test]
fn schema_34_runs_drop_their_private_session_and_automations_gain_an_origin() {
    let directory = tempfile::tempdir().expect("temporary Host catalog");
    let database = directory.path().join("host.sqlite3");
    schema_34_with_runs(&database);

    initialize(&database).expect("upgrade schema 34 in place");
    initialize(&database).expect("reopening the upgraded catalog is stable");
    let connection = open_verified(&database).expect("open upgraded catalog");
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM pragma_table_info('host_automation_runs')
             WHERE name = 'session_id'"
        ),
        0,
        "a run no longer names a private session"
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM host_automations
             WHERE id = '00000000-0000-0000-0000-00000000000a' AND origin_session_id IS NULL"
        ),
        1,
        "an automation from before schema 35 has no recorded origin"
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM host_automation_runs WHERE output IS NULL"
        ),
        1,
        "the admitted run is still pending"
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM host_automation_runs WHERE output = 'Done.'"
        ),
        1,
        "the finished run keeps its result"
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'host_automation_pending'"
        ),
        1
    );
}

#[test]
fn schema_35_gains_a_record_of_failing_registry_synchronization() {
    let directory = tempfile::tempdir().expect("temporary Host catalog");
    let database = directory.path().join("host.sqlite3");
    initialize(&database).expect("initialize current catalog");
    let connection = open_verified(&database).expect("open current catalog");
    restore_schema_35(&connection);
    connection
        .execute_batch(
            "UPDATE host_metadata SET schema_version = 35 WHERE singleton = 1;
             PRAGMA user_version = 35;",
        )
        .expect("mark the catalog as schema 35");
    drop(connection);

    initialize(&database).expect("upgrade schema 35 in place");
    initialize(&database).expect("reopening the upgraded catalog is stable");
    let connection = open_verified(&database).expect("open upgraded catalog");
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM shared_plugin_registry_sync"
        ),
        0,
        "an upgraded Host records no failure until a synchronization fails"
    );
}

#[test]
fn schema_36_runs_gain_a_status_derived_from_their_recorded_result() {
    let directory = tempfile::tempdir().expect("temporary Host catalog");
    let database = directory.path().join("host.sqlite3");
    initialize(&database).expect("initialize current catalog");
    let connection = open_verified(&database).expect("open current catalog");
    restore_schema_36(&connection);
    connection
        .execute_batch(
            "INSERT INTO host_agents(agent_id, name, created_at_ms, created_via,
                preset_id, operational_json, creator_kind, creator_component)
             VALUES ('00000000-0000-0000-0000-000000000001', 'Scheduler', 1,
                'provisioning', NULL, '{}', 'system', 'migration-test');
             INSERT INTO host_automations(id, agent_id, name, prompt, schedule_json,
                enabled, revision, next_due_ms)
             VALUES ('00000000-0000-0000-0000-00000000000a',
                '00000000-0000-0000-0000-000000000001', 'Morning', 'Summarize.',
                '{\"kind\":\"once\",\"at\":\"1970-01-01T00:00:00Z\"}', 1, 2, 100);
             INSERT INTO host_automation_runs(id, automation_id, agent_id, due_ms,
                admitted_at_ms, prompt, output)
             VALUES
                ('00000000-0000-0000-0000-0000000000b1', '00000000-0000-0000-0000-00000000000a',
                 '00000000-0000-0000-0000-000000000001', 1, 1, 'Summarize.', 'Done.'),
                ('00000000-0000-0000-0000-0000000000b2', '00000000-0000-0000-0000-00000000000a',
                 '00000000-0000-0000-0000-000000000001', 2, 2, 'Summarize.',
                 'Scheduled run failed: model unavailable'),
                ('00000000-0000-0000-0000-0000000000b3', '00000000-0000-0000-0000-00000000000a',
                 '00000000-0000-0000-0000-000000000001', 3, 3, 'Summarize.',
                 'Scheduled run stopped. If account setup is needed, resolve it interactively.'),
                ('00000000-0000-0000-0000-0000000000b4', '00000000-0000-0000-0000-00000000000a',
                 '00000000-0000-0000-0000-000000000001', 4, 4, 'Summarize.',
                 'Scheduled run could not be sent: node offline'),
                ('00000000-0000-0000-0000-0000000000b5', '00000000-0000-0000-0000-00000000000a',
                 '00000000-0000-0000-0000-000000000001', 5, 5, 'Summarize.',
                 'Scheduled run needs input. Continue with the agent to resolve it.'),
                ('00000000-0000-0000-0000-0000000000b6', '00000000-0000-0000-0000-00000000000a',
                 '00000000-0000-0000-0000-000000000001', 6, 6, 'Summarize.', NULL);
             UPDATE host_metadata SET schema_version = 36 WHERE singleton = 1;
             PRAGMA user_version = 36;",
        )
        .expect("seed schema 36 runs");
    drop(connection);

    initialize(&database).expect("upgrade schema 36 in place");
    initialize(&database).expect("reopening the upgraded catalog is stable");
    let connection = open_verified(&database).expect("open upgraded catalog");
    let mut statement = connection
        .prepare("SELECT status FROM host_automation_runs ORDER BY due_ms")
        .expect("prepare");
    let statuses = statement
        .query_map([], |row| row.get::<_, Option<String>>(0))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect::<Vec<_>>();
    assert_eq!(
        statuses,
        [
            Some("succeeded"),
            Some("failed"),
            Some("failed"),
            Some("failed"),
            Some("failed"),
            None
        ]
        .map(|status| status.map(str::to_owned)),
        "a finished run's status comes from its text; an unfinished run has none"
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM host_automation_runs
             WHERE failed_tool_calls IS NOT NULL OR finished_at_ms IS NOT NULL"
        ),
        0,
        "what an earlier scheduler did not record stays unknown"
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM host_automation_scheduler"
        ),
        0
    );
}

/// A catalog in the schema 37 shape holding one automation with `schedule`
/// and one unfinished run.
fn schema_37_with(database: &std::path::Path, schedule: &str) {
    initialize(database).expect("initialize current catalog");
    let connection = open_verified(database).expect("open current catalog");
    restore_schema_37(&connection);
    connection
        .execute_batch(&format!(
            "INSERT INTO host_agents(agent_id, name, created_at_ms, created_via,
                preset_id, operational_json, creator_kind, creator_component)
             VALUES ('00000000-0000-0000-0000-000000000001', 'Scheduler', 1,
                'provisioning', NULL, '{{}}', 'system', 'migration-test');
             INSERT INTO host_automations(id, agent_id, name, prompt, schedule_json,
                enabled, revision, next_due_ms)
             VALUES ('00000000-0000-0000-0000-00000000000a',
                '00000000-0000-0000-0000-000000000001', 'Morning', 'Summarize.',
                '{schedule}', 1, 1, 100);
             INSERT INTO host_automation_runs(id, automation_id, agent_id, due_ms,
                admitted_at_ms, prompt)
             VALUES ('00000000-0000-0000-0000-0000000000b1',
                '00000000-0000-0000-0000-00000000000a',
                '00000000-0000-0000-0000-000000000001', 1, 1, 'Summarize.');
             UPDATE host_metadata SET schema_version = 37 WHERE singleton = 1;
             PRAGMA user_version = 37;"
        ))
        .expect("seed schema 37");
}

#[test]
fn schema_37_runs_keep_the_task_they_were_sent() {
    let directory = tempfile::tempdir().expect("temporary Host catalog");
    let database = directory.path().join("host.sqlite3");
    schema_37_with(
        &database,
        r#"{"kind":"cron","expression":"0 9 * * *","timezone":"Asia/Kolkata"}"#,
    );

    initialize(&database).expect("upgrade schema 37 in place");
    initialize(&database).expect("reopening the upgraded catalog is stable");
    let connection = open_verified(&database).expect("open upgraded catalog");
    assert_eq!(
        connection
            .query_row("SELECT submission FROM host_automation_runs", [], |row| {
                row.get::<_, String>(0)
            })
            .expect("run"),
        "Summarize.",
        "a run admitted before schema 38 was sent its standing task alone"
    );
}

#[test]
fn schema_37_daily_and_interval_schedules_are_refused_without_residue() {
    for schedule in [
        r#"{"kind":"daily","hour":9,"minute":0,"timezone":"Asia/Kolkata"}"#,
        r#"{"kind":"interval","hours":6}"#,
    ] {
        let directory = tempfile::tempdir().expect("temporary Host catalog");
        let database = directory.path().join("host.sqlite3");
        schema_37_with(&database, schedule);

        let error = initialize(&database).expect_err("a legacy schedule has no cron form");
        assert!(
            error.to_string().contains(
                "1 stored automations or automation receipts use daily or interval schedules"
            ),
            "{error}"
        );
        let connection = rusqlite::Connection::open(&database).expect("open refused catalog");
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .expect("version"),
            37
        );
        assert_eq!(
            count(
                &connection,
                "SELECT COUNT(*) FROM pragma_table_info('host_automation_runs') WHERE name = 'prompt'"
            ),
            1,
            "the refused upgrade changed nothing"
        );
    }

    let directory = tempfile::tempdir().expect("temporary Host catalog");
    let database = directory.path().join("host.sqlite3");
    schema_37_with(&database, r#"{"kind":"once","at":"1970-01-01T00:00:00Z"}"#);
    rusqlite::Connection::open(&database)
        .expect("open catalog")
        .execute(
            "INSERT INTO host_automation_mutations VALUES ('00000000-0000-0000-0000-00000000000c',
                '00000000-0000-0000-0000-000000000001', '{}', ?1)",
            [r#"{"spec":{"schedule":{"kind":"interval","hours":6}}}"#],
        )
        .expect("seed a legacy receipt");
    assert!(
        initialize(&database).is_err(),
        "a receipt that replays a legacy schedule is refused too"
    );
}

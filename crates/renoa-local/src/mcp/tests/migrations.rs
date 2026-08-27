use std::{
    path::Path,
    sync::{Arc, Barrier},
    thread,
};

use rusqlite::Connection;
use tempfile::tempdir;

use super::{ENDPOINT, PROFILE, snapshot, store};
use crate::{
    host::catalog::HostCatalogError,
    mcp::{McpCatalogStore, McpConnectionAuth, McpHostError},
};

#[test]
fn a_newer_host_catalog_schema_is_rejected() {
    let (directory, store) = store();
    let path = store.path().to_owned();
    drop(store);
    Connection::open(&path)
        .expect("open schema mutation connection")
        .pragma_update(None, "user_version", 6_u32)
        .expect("advance schema version");

    assert!(matches!(
        McpCatalogStore::initialize(directory.path().join("host.sqlite3")),
        Err(McpHostError::HostCatalog(HostCatalogError::Invalid(_)))
    ));
}

#[test]
fn version_one_catalog_migrates_without_losing_no_auth_state() {
    let (directory, store) = store();
    store
        .register_direct_connection("example", "primary", ENDPOINT)
        .expect("register connection");
    store
        .publish_catalog(&snapshot("primary", ENDPOINT, &["echo"]))
        .expect("publish catalog");
    store
        .enable_alpha_connection(PROFILE, "primary")
        .expect("enable connection");
    let path = store.path().to_owned();
    drop(store);

    let connection = Connection::open(&path).expect("open migration fixture");
    connection
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             CREATE TABLE profile_mcp_tools (
                profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
                connection_id TEXT NOT NULL
                    REFERENCES mcp_connections(connection_id) ON DELETE RESTRICT,
                tool_name TEXT NOT NULL CHECK (length(tool_name) > 0),
                PRIMARY KEY (profile_id, connection_id, tool_name)
             ) STRICT;
             INSERT INTO profile_mcp_tools(profile_id, connection_id, tool_name)
             VALUES ('renoa.coding.alpha.v1', 'primary', 'echo');
             DROP TABLE profile_mcp_connections;
             DROP TABLE session_skills;
             DROP TABLE skill_source_rejections;
             DROP TABLE profile_skill_bindings;
             DROP TABLE skill_revisions;
             CREATE TABLE mcp_connections_v1 (
                connection_id TEXT PRIMARY KEY CHECK (length(connection_id) > 0),
                integration_id TEXT NOT NULL REFERENCES mcp_integrations(integration_id),
                auth_kind TEXT NOT NULL CHECK (auth_kind = 'none')
             ) STRICT;
             INSERT INTO mcp_connections_v1(connection_id, integration_id, auth_kind)
             SELECT connection_id, integration_id, auth_kind FROM mcp_connections;
             DROP TABLE mcp_connections;
             ALTER TABLE mcp_connections_v1 RENAME TO mcp_connections;
             UPDATE host_metadata SET schema_version = 1 WHERE singleton = 1;
             PRAGMA user_version = 1;",
        )
        .expect("downgrade fixture to schema v1");
    drop(connection);

    let migrated = McpCatalogStore::initialize(directory.path().join("host.sqlite3"))
        .expect("migrate schema v1 to v4");
    assert_eq!(
        migrated
            .connection_config("primary")
            .expect("load migrated connection")
            .auth,
        McpConnectionAuth::None
    );
    assert_eq!(
        migrated
            .alpha_connection_ids(PROFILE)
            .expect("load migrated Alpha connections"),
        ["primary"]
    );
    assert_eq!(
        migrated
            .alpha_tool_summaries(PROFILE)
            .expect("load migrated Alpha tools")[0]
            .name,
        "echo"
    );
}

#[test]
fn version_two_any_tool_selection_migrates_to_the_full_connection_attachment() {
    let (directory, store) = store();
    store
        .register_direct_connection("example", "primary", ENDPOINT)
        .expect("register connection");
    store
        .publish_catalog(&snapshot("primary", ENDPOINT, &["echo", "unused"]))
        .expect("publish catalog");
    let path = store.path().to_owned();
    drop(store);

    Connection::open(&path)
        .expect("open migration fixture")
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             CREATE TABLE profile_mcp_tools (
                profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
                connection_id TEXT NOT NULL
                    REFERENCES mcp_connections(connection_id) ON DELETE RESTRICT,
                tool_name TEXT NOT NULL CHECK (length(tool_name) > 0),
                PRIMARY KEY (profile_id, connection_id, tool_name)
             ) STRICT;
             INSERT INTO profile_mcp_tools(profile_id, connection_id, tool_name)
             VALUES ('renoa.coding.alpha.v1', 'primary', 'echo');
             DROP TABLE profile_mcp_connections;
             DROP TABLE session_skills;
             DROP TABLE skill_source_rejections;
             DROP TABLE profile_skill_bindings;
             DROP TABLE skill_revisions;
             UPDATE host_metadata SET schema_version = 2 WHERE singleton = 1;
             PRAGMA user_version = 2;",
        )
        .expect("downgrade fixture to schema v2");

    let migrated = McpCatalogStore::initialize(directory.path().join("host.sqlite3"))
        .expect("migrate schema v2 to v4");
    assert_eq!(
        migrated
            .alpha_connection_ids(PROFILE)
            .expect("load migrated Alpha connections"),
        ["primary"]
    );
    assert_eq!(
        migrated
            .alpha_tool_summaries(PROFILE)
            .expect("load the full attached catalog")
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["echo", "unused"]
    );
}

#[test]
fn version_three_catalog_adds_current_skill_state_without_changing_mcp_state() {
    let (directory, store) = store();
    store
        .register_direct_connection("example", "primary", ENDPOINT)
        .expect("register connection");
    store
        .publish_catalog(&snapshot("primary", ENDPOINT, &["echo"]))
        .expect("publish catalog");
    store
        .enable_alpha_connection(PROFILE, "primary")
        .expect("enable connection");
    let path = store.path().to_owned();
    drop(store);

    Connection::open(&path)
        .expect("open migration fixture")
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             DROP TABLE session_skills;
             DROP TABLE skill_source_rejections;
             DROP TABLE profile_skill_bindings;
             DROP TABLE skill_revisions;
             UPDATE host_metadata SET schema_version = 3 WHERE singleton = 1;
             PRAGMA user_version = 3;",
        )
        .expect("downgrade fixture to schema v3");

    let migrated = McpCatalogStore::initialize(directory.path().join("host.sqlite3"))
        .expect("migrate schema v3 to current");
    assert_eq!(
        migrated
            .alpha_connection_ids(PROFILE)
            .expect("load migrated Alpha connections"),
        ["primary"]
    );
    assert_eq!(
        migrated
            .alpha_tool_summaries(PROFILE)
            .expect("load migrated Alpha tools")[0]
            .name,
        "echo"
    );
    Connection::open(migrated.path())
        .expect("open migrated catalog")
        .prepare("SELECT activation_command_id FROM session_skills")
        .expect("current session skills include command ownership");
}

#[test]
fn version_four_catalog_removes_instruction_policy_without_losing_activations() {
    let (_directory, store) = store();
    let path = store.path().to_owned();
    drop(store);

    downgrade_to_v4_with_large_skill(&path);

    let migrated = McpCatalogStore::initialize(path).expect("migrate schema v4 to current");
    let connection = Connection::open(migrated.path()).expect("open migrated catalog");
    let columns = connection
        .prepare("PRAGMA table_info(session_skills)")
        .expect("prepare session skill columns")
        .query_map([], |row| row.get::<_, String>(1))
        .expect("query session skill columns")
        .collect::<Result<Vec<_>, _>>()
        .expect("read session skill columns");
    assert!(!columns.iter().any(|column| column == "instruction_bytes"));
    assert_eq!(
        connection
            .query_row(
                "SELECT activation_order, session_id, activation_command_id, skill_name,
                        skill_digest
                 FROM session_skills",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .expect("read migrated activation"),
        (
            7,
            "session".to_owned(),
            "command".to_owned(),
            "large".to_owned(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        )
    );
    connection
        .execute_batch(
            "INSERT INTO skill_revisions(
                skill_digest, name, description, license, compatibility
             ) VALUES (
                'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                'next', 'Next skill.', NULL, NULL
             );
             INSERT INTO session_skills(
                session_id, activation_command_id, skill_name, skill_digest
             ) VALUES (
                'session', 'next-command', 'next',
                'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'
             );",
        )
        .expect("insert activation after migration");
    assert_eq!(
        connection
            .query_row(
                "SELECT activation_order FROM session_skills WHERE skill_name = 'next'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("read next activation order"),
        8
    );
}

fn downgrade_to_v4_with_large_skill(path: &Path) {
    Connection::open(path)
        .expect("open migration fixture")
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             DROP TABLE session_skills;
             CREATE TABLE session_skills (
                activation_order INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL CHECK (length(session_id) > 0),
                activation_command_id TEXT NOT NULL CHECK (length(activation_command_id) > 0),
                skill_name TEXT NOT NULL CHECK (length(skill_name) > 0),
                skill_digest TEXT NOT NULL,
                instruction_bytes INTEGER NOT NULL CHECK (instruction_bytes > 0),
                FOREIGN KEY (skill_digest, skill_name)
                    REFERENCES skill_revisions(skill_digest, name) ON DELETE RESTRICT,
                UNIQUE (session_id, skill_name),
                UNIQUE (session_id, skill_digest)
             ) STRICT;
             INSERT INTO skill_revisions(
                skill_digest, name, description, license, compatibility
             ) VALUES (
                'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                'large', 'Large skill.', NULL, NULL
             );
             INSERT INTO session_skills(
                activation_order, session_id, activation_command_id, skill_name,
                skill_digest, instruction_bytes
             ) VALUES (
                7, 'session', 'command', 'large',
                'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                102401
             );
             UPDATE host_metadata SET schema_version = 4 WHERE singleton = 1;
             PRAGMA user_version = 4;",
        )
        .expect("downgrade fixture to schema v4");
}

#[test]
fn concurrent_first_initialization_publishes_one_valid_schema() {
    let directory = tempdir().expect("temporary Host data directory");
    let path = directory.path().join("host.sqlite3");
    let barrier = Arc::new(Barrier::new(4));
    let workers = (0..4)
        .map(|_| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                McpCatalogStore::initialize(path)
            })
        })
        .collect::<Vec<_>>();

    for worker in workers {
        worker
            .join()
            .expect("Host initialization thread")
            .expect("concurrent Host initialization");
    }
    McpCatalogStore::initialize(path).expect("reopen concurrently initialized catalog");
}

#[cfg(unix)]
#[test]
fn host_catalog_database_is_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;

    let (_directory, store) = store();
    let mode = std::fs::metadata(store.path())
        .expect("Host catalog metadata")
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(mode, 0o600);
}

use std::path::Path;

use rusqlite::Connection;

use super::super::{PROFILE, store};
use crate::mcp::McpCatalogStore;

#[test]
fn version_six_catalog_adds_plugin_skill_scope_without_losing_existing_bindings() {
    let (directory, store) = store();
    let path = store.path().to_owned();
    drop(store);
    let digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let connection = Connection::open(&path).expect("open migration fixture");
    connection
        .execute_batch(&format!(
            "PRAGMA foreign_keys = OFF;
             DROP TABLE shared_plugin_registry_state;
             DROP TABLE mcp_oauth_receipts;
             DROP TABLE mcp_oauth_flows;
             ALTER TABLE installed_plugins DROP COLUMN homepage;
             INSERT INTO skill_revisions(
                skill_digest, name, description, license, compatibility
             ) VALUES ('{digest}', 'review', 'Review code.', NULL, NULL);
             INSERT INTO profile_skill_bindings(
                profile_id, scope_kind, workspace, source_id, skill_name, skill_digest
             ) VALUES ('{PROFILE}', 'global', NULL, '/skills', 'review', '{digest}');
             CREATE TABLE profile_skill_bindings_v6 (
                profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
                scope_kind TEXT NOT NULL CHECK (scope_kind IN ('global', 'workspace')),
                workspace TEXT,
                source_root TEXT NOT NULL CHECK (length(source_root) > 0),
                skill_name TEXT NOT NULL CHECK (length(skill_name) > 0),
                skill_digest TEXT NOT NULL,
                FOREIGN KEY (skill_digest, skill_name)
                    REFERENCES skill_revisions(skill_digest, name) ON DELETE RESTRICT,
                CHECK (
                    (scope_kind = 'global' AND workspace IS NULL)
                    OR
                    (scope_kind = 'workspace' AND length(workspace) > 0)
                ),
                PRIMARY KEY (profile_id, source_root, skill_name)
             ) STRICT;
             INSERT INTO profile_skill_bindings_v6
             SELECT profile_id, scope_kind, workspace, source_id, skill_name, skill_digest
             FROM profile_skill_bindings;
             DROP TABLE profile_skill_bindings;
             ALTER TABLE profile_skill_bindings_v6 RENAME TO profile_skill_bindings;
             CREATE TABLE skill_source_rejections_v6 (
                profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
                scope_kind TEXT NOT NULL CHECK (scope_kind IN ('global', 'workspace')),
                workspace TEXT,
                source_root TEXT NOT NULL CHECK (length(source_root) > 0),
                entry_name TEXT NOT NULL CHECK (length(entry_name) > 0),
                reason TEXT NOT NULL CHECK (length(reason) > 0),
                CHECK (
                    (scope_kind = 'global' AND workspace IS NULL)
                    OR
                    (scope_kind = 'workspace' AND length(workspace) > 0)
                ),
                PRIMARY KEY (profile_id, source_root, entry_name)
             ) STRICT;
             INSERT INTO skill_source_rejections_v6
             SELECT profile_id, scope_kind, workspace, source_id, entry_name, reason
             FROM skill_source_rejections;
             DROP TABLE skill_source_rejections;
             ALTER TABLE skill_source_rejections_v6 RENAME TO skill_source_rejections;
             UPDATE host_metadata SET schema_version = 6 WHERE singleton = 1;
             PRAGMA user_version = 6;"
        ))
        .expect("downgrade fixture to schema v6");
    drop(connection);

    let migrated = McpCatalogStore::initialize(directory.path().join("host.sqlite3"))
        .expect("migrate schema v6 to current");
    let connection = Connection::open(migrated.path()).expect("open migrated catalog");
    assert_eq!(
        connection
            .query_row(
                "SELECT scope_kind, source_id, skill_digest
                 FROM profile_skill_bindings WHERE skill_name = 'review'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .expect("read migrated skill binding"),
        ("global".to_owned(), "/skills".to_owned(), digest.to_owned())
    );
    connection
        .execute(
            "INSERT INTO profile_skill_bindings(
                profile_id, scope_kind, workspace, source_id, skill_name, skill_digest
             ) VALUES (?1, 'plugin', NULL, 'agent-plugin:fixture', 'review', ?2)",
            [PROFILE, digest],
        )
        .expect("new schema accepts plugin skill scope");
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
    let connection = Connection::open(path).expect("open migration fixture");
    downgrade_skill_sources_to_v6_shape(&connection);
    connection
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             DROP TABLE shared_plugin_registry_state;
             DROP TABLE mcp_oauth_receipts;
             DROP TABLE mcp_oauth_flows;
             DROP TABLE plugin_mcp_servers;
             DROP TABLE installed_plugins;
             ALTER TABLE mcp_catalogs DROP COLUMN request_headers_json;
             ALTER TABLE mcp_integrations DROP COLUMN request_headers_json;
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

pub(super) fn downgrade_skill_sources_to_v6_shape(connection: &Connection) {
    connection
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             CREATE TABLE profile_skill_bindings_v6 (
                profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
                scope_kind TEXT NOT NULL CHECK (scope_kind IN ('global', 'workspace')),
                workspace TEXT,
                source_root TEXT NOT NULL CHECK (length(source_root) > 0),
                skill_name TEXT NOT NULL CHECK (length(skill_name) > 0),
                skill_digest TEXT NOT NULL,
                FOREIGN KEY (skill_digest, skill_name)
                    REFERENCES skill_revisions(skill_digest, name) ON DELETE RESTRICT,
                CHECK (
                    (scope_kind = 'global' AND workspace IS NULL)
                    OR
                    (scope_kind = 'workspace' AND length(workspace) > 0)
                ),
                PRIMARY KEY (profile_id, source_root, skill_name)
             ) STRICT;
             INSERT INTO profile_skill_bindings_v6
             SELECT profile_id, scope_kind, workspace, source_id, skill_name, skill_digest
             FROM profile_skill_bindings;
             DROP TABLE profile_skill_bindings;
             ALTER TABLE profile_skill_bindings_v6 RENAME TO profile_skill_bindings;
             CREATE TABLE skill_source_rejections_v6 (
                profile_id TEXT NOT NULL CHECK (length(profile_id) > 0),
                scope_kind TEXT NOT NULL CHECK (scope_kind IN ('global', 'workspace')),
                workspace TEXT,
                source_root TEXT NOT NULL CHECK (length(source_root) > 0),
                entry_name TEXT NOT NULL CHECK (length(entry_name) > 0),
                reason TEXT NOT NULL CHECK (length(reason) > 0),
                CHECK (
                    (scope_kind = 'global' AND workspace IS NULL)
                    OR
                    (scope_kind = 'workspace' AND length(workspace) > 0)
                ),
                PRIMARY KEY (profile_id, source_root, entry_name)
             ) STRICT;
             INSERT INTO skill_source_rejections_v6
             SELECT profile_id, scope_kind, workspace, source_id, entry_name, reason
             FROM skill_source_rejections;
             DROP TABLE skill_source_rejections;
             ALTER TABLE skill_source_rejections_v6 RENAME TO skill_source_rejections;",
        )
        .expect("downgrade skill source tables to schema v6 shape");
}

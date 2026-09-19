use std::path::Path;

use rusqlite::Connection;

use super::{LEGACY_PROFILE_ID, count, cut_over, retired, store};

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
             INSERT INTO agent_skill_bindings(
                agent_id, scope_kind, workspace, source_id, skill_name, skill_digest
             ) VALUES ('{LEGACY_PROFILE_ID}', 'global', NULL, '/skills', 'review', '{digest}');
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
             SELECT agent_id, scope_kind, workspace, source_id, skill_name, skill_digest
             FROM agent_skill_bindings;
             DROP TABLE agent_skill_bindings;
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
             SELECT agent_id, scope_kind, workspace, source_id, entry_name, reason
             FROM agent_skill_source_rejections;
             DROP TABLE agent_skill_source_rejections;
             ALTER TABLE skill_source_rejections_v6 RENAME TO skill_source_rejections;
             DROP TABLE host_agents; DROP TABLE host_identity;
             UPDATE host_metadata SET schema_version = 6 WHERE singleton = 1;
             PRAGMA user_version = 6;"
        ))
        .expect("downgrade fixture to schema v6");
    drop(connection);

    let migrated = cut_over(directory.path());
    let connection = Connection::open(migrated.path()).expect("open cut-over catalog");
    assert!(
        retired(&connection, "profile_skill_bindings"),
        "the retired profile binding table must not survive the cutover"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT name FROM skill_revisions WHERE skill_digest = ?1",
                [digest],
                |row| row.get::<_, String>(0),
            )
            .expect("retained skill revision"),
        "review"
    );
    assert_eq!(
        count(&connection, "agent_skill_bindings"),
        0,
        "the cutover must not fabricate an agent binding"
    );
    connection
        .execute(
            "INSERT INTO agent_skill_bindings(
                agent_id, scope_kind, workspace, source_id, skill_name, skill_digest
             ) VALUES ('agent-after-cutover', 'plugin', NULL, 'agent-plugin:fixture', 'review', ?1)",
            [digest],
        )
        .expect("canonical schema accepts plugin skill scope");
}

#[test]
fn version_four_catalog_removes_instruction_policy_without_losing_activations() {
    let (directory, store) = store();
    let path = store.path().to_owned();
    drop(store);

    downgrade_to_v4_with_large_skill(&path);

    let migrated = cut_over(directory.path());
    let connection = Connection::open(migrated.path()).expect("open cut-over catalog");
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
                "SELECT name FROM skill_revisions
                 WHERE skill_digest = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("retained skill revision"),
        "large"
    );
    assert_eq!(
        count(&connection, "session_skills"),
        0,
        "the cutover discards session activations"
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
        .expect("insert activation after the cutover");
    assert_eq!(
        connection
            .query_row(
                "SELECT activation_order FROM session_skills WHERE skill_name = 'next'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .expect("read next activation order"),
        1
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
             DROP TABLE host_agents; DROP TABLE host_identity;
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
             SELECT agent_id, scope_kind, workspace, source_id, skill_name, skill_digest
             FROM agent_skill_bindings;
             DROP TABLE agent_skill_bindings;
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
             SELECT agent_id, scope_kind, workspace, source_id, entry_name, reason
             FROM agent_skill_source_rejections;
             DROP TABLE agent_skill_source_rejections;
             ALTER TABLE skill_source_rejections_v6 RENAME TO skill_source_rejections;",
        )
        .expect("downgrade skill source tables to schema v6 shape");
}

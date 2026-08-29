use rusqlite::Connection;

use super::super::{PROFILE, snapshot, store};
use crate::mcp::{McpCatalogStore, McpConnectionAuth, McpOAuthRegistration, McpRequestHeaders};

#[test]
fn version_eight_oauth_connections_migrate_as_dynamic_registration() {
    let (directory, store) = store();
    let path = store.path().to_owned();
    let oauth = McpConnectionAuth::oauth(
        "oauth",
        "https://example.com/oauth-mcp",
        McpOAuthRegistration::dynamic(),
    )
    .expect("dynamic OAuth reference");
    store
        .register_connection(
            "oauth-integration",
            "oauth",
            "https://example.com/oauth-mcp",
            &McpRequestHeaders::default(),
            &oauth,
        )
        .expect("register current OAuth connection");
    store
        .publish_and_enable_connection(
            PROFILE,
            &snapshot("oauth", "https://example.com/oauth-mcp", &["search"]),
        )
        .expect("publish current OAuth catalog");
    drop(store);

    let connection = Connection::open(&path).expect("open version downgrade fixture");
    connection
        .execute_batch(
            r#"PRAGMA foreign_keys = OFF;
             INSERT INTO mcp_oauth_flows(
                connection_id, operation_id, phase, callback_port, expires_at_ms
             ) VALUES ('oauth', 'migration-flow', 'unknown', NULL, NULL);
             INSERT INTO mcp_oauth_receipts(connection_id, operation_id, outcome_json)
             VALUES ('oauth', 'migration-receipt', '{"status":"failed"}');
             CREATE TABLE mcp_connections_v8 (
                connection_id TEXT PRIMARY KEY CHECK (length(connection_id) > 0),
                integration_id TEXT NOT NULL REFERENCES mcp_integrations(integration_id),
                auth_kind TEXT NOT NULL CHECK (
                    auth_kind IN ('none', 'gh_cli', 'secret_service_bearer', 'oauth')
                ),
                auth_hostname TEXT,
                auth_account TEXT,
                auth_credential_id TEXT,
                CHECK (
                    (auth_kind = 'none' AND auth_hostname IS NULL AND auth_account IS NULL
                     AND auth_credential_id IS NULL)
                    OR
                    (auth_kind = 'gh_cli' AND length(auth_hostname) > 0
                     AND length(auth_account) > 0 AND auth_credential_id IS NULL)
                    OR
                    (auth_kind IN ('secret_service_bearer', 'oauth')
                     AND auth_hostname IS NULL AND auth_account IS NULL
                     AND length(auth_credential_id) > 0)
                )
             ) STRICT;
             INSERT INTO mcp_connections_v8(
                connection_id, integration_id, auth_kind, auth_hostname, auth_account,
                auth_credential_id
             )
             SELECT connection_id, integration_id, auth_kind, auth_hostname, auth_account,
                    auth_credential_id
             FROM mcp_connections;
             DROP TABLE mcp_connections;
             ALTER TABLE mcp_connections_v8 RENAME TO mcp_connections;
             UPDATE host_metadata SET schema_version = 8 WHERE singleton = 1;
             PRAGMA user_version = 8;"#,
        )
        .expect("downgrade fixture to version eight");
    drop(connection);

    let migrated = McpCatalogStore::initialize(directory.path().join("host.sqlite3"))
        .expect("migrate version eight OAuth connection");
    assert_eq!(
        migrated
            .connection_config("oauth")
            .expect("load migrated OAuth connection")
            .auth,
        oauth
    );
    assert_eq!(
        migrated
            .alpha_tool_summaries(PROFILE)
            .expect("load migrated attachment")
            .len(),
        1
    );
    let connection = Connection::open(migrated.path()).expect("inspect migrated OAuth state");
    for table in ["mcp_oauth_flows", "mcp_oauth_receipts"] {
        let count = connection
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE connection_id = 'oauth'"),
                [],
                |row| row.get::<_, u32>(0),
            )
            .expect("count preserved OAuth state");
        assert_eq!(count, 1, "{table} must survive migration");
    }
}

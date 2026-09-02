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
             DROP TABLE shared_plugin_registry_state;
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
            .profile_tool_summaries(PROFILE)
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

#[test]
fn version_nine_credentials_survive_and_custom_headers_become_available() {
    let (directory, store) = store();
    let path = store.path().to_owned();
    let bearer = McpConnectionAuth::secret_service_bearer("existing.bearer")
        .expect("valid bearer reference");
    store
        .register_connection(
            "existing",
            "existing",
            "https://example.com/existing-mcp",
            &McpRequestHeaders::default(),
            &bearer,
        )
        .expect("register existing credential");
    drop(store);

    Connection::open(&path)
        .expect("open version downgrade fixture")
        .execute_batch(
            r"PRAGMA foreign_keys = OFF;
             DROP TABLE shared_plugin_registry_state;
             CREATE TABLE mcp_connections_v9 (
                connection_id TEXT PRIMARY KEY CHECK (length(connection_id) > 0),
                integration_id TEXT NOT NULL REFERENCES mcp_integrations(integration_id),
                auth_kind TEXT NOT NULL CHECK (
                    auth_kind IN ('none', 'gh_cli', 'secret_service_bearer', 'oauth')
                ),
                auth_hostname TEXT,
                auth_account TEXT,
                auth_credential_id TEXT,
                oauth_registration_json TEXT
             ) STRICT;
             INSERT INTO mcp_connections_v9(
                connection_id, integration_id, auth_kind, auth_hostname, auth_account,
                auth_credential_id, oauth_registration_json
             )
             SELECT connection_id, integration_id, auth_kind, auth_hostname, auth_account,
                    auth_credential_id, oauth_registration_json
             FROM mcp_connections;
             DROP TABLE mcp_connections;
             ALTER TABLE mcp_connections_v9 RENAME TO mcp_connections;
             UPDATE host_metadata SET schema_version = 9 WHERE singleton = 1;
             PRAGMA user_version = 9;",
        )
        .expect("downgrade fixture to version nine");

    let migrated = McpCatalogStore::initialize(directory.path().join("host.sqlite3"))
        .expect("migrate version nine credential state");
    assert_eq!(
        migrated
            .connection_config("existing")
            .expect("load migrated bearer credential")
            .auth,
        bearer
    );

    let custom = McpConnectionAuth::secret_service_header("exa.api-key", "X-API-Key", "ApiKey ")
        .expect("valid custom header reference");
    migrated
        .register_connection(
            "custom",
            "custom",
            "https://example.com/custom-mcp",
            &McpRequestHeaders::default(),
            &custom,
        )
        .expect("register custom credential after migration");
    assert_eq!(
        migrated
            .connection_config("custom")
            .expect("load custom credential")
            .auth,
        custom
    );
}

#[test]
fn version_eleven_loopback_oauth_flow_gains_an_empty_relay_identity() {
    let (directory, store) = store();
    let path = store.path().to_owned();
    let oauth = McpConnectionAuth::oauth(
        "oauth-v11",
        "https://example.com/oauth-mcp",
        McpOAuthRegistration::dynamic(),
    )
    .expect("dynamic OAuth reference");
    store
        .register_connection(
            "oauth-v11-integration",
            "oauth-v11",
            "https://example.com/oauth-mcp",
            &McpRequestHeaders::default(),
            &oauth,
        )
        .expect("register OAuth connection");
    drop(store);

    Connection::open(&path)
        .expect("open version downgrade fixture")
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             CREATE TABLE mcp_oauth_flows_v11 (
                 connection_id TEXT PRIMARY KEY
                     REFERENCES mcp_connections(connection_id) ON DELETE CASCADE,
                 operation_id TEXT NOT NULL CHECK (length(operation_id) BETWEEN 1 AND 512),
                 phase TEXT NOT NULL CHECK (
                     phase IN (
                         'begin_in_flight', 'awaiting_callback', 'callback_ready',
                         'exchange_in_flight', 'refresh_in_flight', 'unknown'
                     )
                 ),
                 callback_port INTEGER,
                 expires_at_ms INTEGER,
                 CHECK (
                     (phase IN ('begin_in_flight', 'awaiting_callback', 'callback_ready',
                                'exchange_in_flight')
                      AND callback_port BETWEEN 1 AND 65535 AND expires_at_ms > 0)
                     OR
                     (phase IN ('refresh_in_flight', 'unknown')
                      AND callback_port IS NULL AND expires_at_ms IS NULL)
                 )
             ) STRICT;
             INSERT INTO mcp_oauth_flows_v11(
                 connection_id, operation_id, phase, callback_port, expires_at_ms
             ) VALUES ('oauth-v11', 'operation-v11', 'awaiting_callback', 43123, 9999999999999);
             DROP TABLE mcp_oauth_flows;
             ALTER TABLE mcp_oauth_flows_v11 RENAME TO mcp_oauth_flows;
             UPDATE host_metadata SET schema_version = 11 WHERE singleton = 1;
             PRAGMA user_version = 11;",
        )
        .expect("downgrade fixture to version eleven");

    let migrated = McpCatalogStore::initialize(directory.path().join("host.sqlite3"))
        .expect("migrate version eleven callback flow");
    let connection = Connection::open(migrated.path()).expect("inspect migrated callback flow");
    let callback = connection
        .query_row(
            "SELECT callback_port, callback_relay_id FROM mcp_oauth_flows
             WHERE connection_id = 'oauth-v11'",
            [],
            |row| Ok((row.get::<_, u16>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .expect("load migrated callback identity");
    assert_eq!(callback, (43123, None));
}

#[test]
fn version_twelve_oauth_attempts_are_decoupled_from_active_connections() {
    let (directory, store) = store();
    let path = store.path().to_owned();
    let oauth = McpConnectionAuth::oauth(
        "oauth-v12",
        "https://example.com/oauth-mcp",
        McpOAuthRegistration::dynamic(),
    )
    .expect("dynamic OAuth reference");
    store
        .register_connection(
            "oauth-v12-integration",
            "oauth-v12",
            "https://example.com/oauth-mcp",
            &McpRequestHeaders::default(),
            &oauth,
        )
        .expect("register OAuth connection");
    drop(store);

    let connection = Connection::open(&path).expect("open version downgrade fixture");
    connection
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             CREATE TABLE mcp_oauth_flows_v12 (
                 connection_id TEXT PRIMARY KEY
                     REFERENCES mcp_connections(connection_id) ON DELETE CASCADE,
                 operation_id TEXT NOT NULL,
                 phase TEXT NOT NULL,
                 callback_port INTEGER,
                 callback_relay_id TEXT,
                 expires_at_ms INTEGER
             ) STRICT;
             INSERT INTO mcp_oauth_flows_v12(
                 connection_id, operation_id, phase, callback_port, callback_relay_id,
                 expires_at_ms
             ) VALUES (
                 'oauth-v12', 'operation-v12', 'awaiting_callback', 43123, NULL,
                 9999999999999
             );
             CREATE TABLE mcp_oauth_receipts_v12 (
                 connection_id TEXT NOT NULL
                     REFERENCES mcp_connections(connection_id) ON DELETE CASCADE,
                 operation_id TEXT NOT NULL,
                 outcome_json TEXT NOT NULL,
                 PRIMARY KEY (connection_id, operation_id)
             ) STRICT;
             INSERT INTO mcp_oauth_receipts_v12(connection_id, operation_id, outcome_json)
             VALUES ('oauth-v12', 'receipt-v12', '{\"outcome\":\"authorized\"}');
             DROP TABLE mcp_oauth_receipts;
             ALTER TABLE mcp_oauth_receipts_v12 RENAME TO mcp_oauth_receipts;
             DROP TABLE mcp_oauth_flows;
             ALTER TABLE mcp_oauth_flows_v12 RENAME TO mcp_oauth_flows;
             UPDATE host_metadata SET schema_version = 12 WHERE singleton = 1;
             PRAGMA user_version = 12;",
        )
        .expect("downgrade fixture to version twelve");
    drop(connection);

    let migrated = McpCatalogStore::initialize(directory.path().join("host.sqlite3"))
        .expect("migrate version twelve OAuth state");
    let connection = Connection::open(migrated.path()).expect("inspect migrated OAuth state");
    let flow_count = connection
        .query_row("SELECT count(*) FROM mcp_oauth_flows", [], |row| {
            row.get::<_, u32>(0)
        })
        .expect("count migrated flows");
    let receipt_count = connection
        .query_row("SELECT count(*) FROM mcp_oauth_receipts", [], |row| {
            row.get::<_, u32>(0)
        })
        .expect("count migrated receipts");
    assert_eq!((flow_count, receipt_count), (1, 1));
    for table in ["mcp_oauth_flows", "mcp_oauth_receipts"] {
        let foreign_keys = connection
            .query_row(
                &format!("SELECT count(*) FROM pragma_foreign_key_list('{table}')"),
                [],
                |row| row.get::<_, u32>(0),
            )
            .expect("count OAuth foreign keys");
        assert_eq!(foreign_keys, 0, "{table} must hold staged OAuth state");
    }
    connection
        .execute(
            "INSERT INTO mcp_oauth_receipts(connection_id, operation_id, outcome_json)
             VALUES ('not-active-yet', 'staged', '{\"outcome\":\"authorized\"}')",
            [],
        )
        .expect("staged OAuth receipt does not require active connection config");
}

use std::{
    sync::{Arc, Barrier},
    thread,
};

use rusqlite::Connection;
use serde_json::json;
use tempfile::tempdir;

use super::{
    AdapterCatalog, MCP_ADAPTER_REVISION, MCP_PROTOCOL_VERSION, McpCatalogSnapshot,
    McpCatalogStore, McpCatalogTool, McpConnectionAuth, McpHostError, McpRejectedTool, hex_sha256,
};

const PROFILE: &str = "renoa.coding.alpha.v1";
const ENDPOINT: &str = "http://127.0.0.1:43127/mcp";

fn store() -> (tempfile::TempDir, McpCatalogStore) {
    let directory = tempdir().expect("temporary Host data directory");
    let store = McpCatalogStore::initialize(directory.path().join("host.sqlite3"))
        .expect("initialize Host catalog");
    (directory, store)
}

fn tool(name: &str) -> McpCatalogTool {
    McpCatalogTool {
        name: name.to_owned(),
        description: format!("{name} description"),
        input_schema: json!({"type": "object"}),
        model_input_schema: json!({"type": "object"}),
        output_schema: Some(json!({"type": "object"})),
    }
}

#[test]
fn catalog_digest_uses_lowercase_sha256() {
    assert_eq!(
        hex_sha256(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn rejected_tool_indexes_must_be_strictly_ordered_for_durable_round_trips() {
    let error = McpCatalogSnapshot::from_adapter(
        "primary",
        AdapterCatalog {
            endpoint: ENDPOINT.to_owned(),
            protocol_version: MCP_PROTOCOL_VERSION.to_owned(),
            adapter_revision: MCP_ADAPTER_REVISION.to_owned(),
            tools: Vec::new(),
            rejected_tools: vec![
                McpRejectedTool {
                    index: 2,
                    name: Some("later".to_owned()),
                    reason: "invalid".to_owned(),
                },
                McpRejectedTool {
                    index: 1,
                    name: Some("earlier".to_owned()),
                    reason: "invalid".to_owned(),
                },
            ],
        },
    )
    .expect_err("storage reconstructs rejected tools in source-index order");

    assert!(matches!(error, McpHostError::Invalid(_)));
}

fn snapshot(connection_id: &str, endpoint: &str, names: &[&str]) -> McpCatalogSnapshot {
    McpCatalogSnapshot::from_adapter(
        connection_id,
        AdapterCatalog {
            endpoint: endpoint.to_owned(),
            protocol_version: MCP_PROTOCOL_VERSION.to_owned(),
            adapter_revision: MCP_ADAPTER_REVISION.to_owned(),
            tools: names.iter().map(|name| tool(name)).collect(),
            rejected_tools: Vec::new(),
        },
    )
    .expect("valid catalog snapshot")
}

#[test]
fn registration_is_idempotent_but_identity_reuse_cannot_change_configuration() {
    let (_directory, store) = store();
    store
        .register_direct_connection("example", "primary", ENDPOINT)
        .expect("register connection");

    store
        .register_direct_connection("example", "primary", ENDPOINT)
        .expect("repeat exact registration");
    let endpoint_conflict = store
        .register_direct_connection("example", "secondary", "http://127.0.0.1:9/mcp")
        .expect_err("integration identity must not change endpoint");
    assert!(matches!(endpoint_conflict, McpHostError::Conflict(_)));

    store
        .register_direct_connection("other", "other", "http://127.0.0.1:10/mcp")
        .expect("register second integration");
    let connection_conflict = store
        .register_direct_connection("other", "primary", "http://127.0.0.1:10/mcp")
        .expect_err("connection identity must not change integration");
    assert!(matches!(connection_conflict, McpHostError::Conflict(_)));
    assert_eq!(
        store
            .connection_endpoint("primary")
            .expect("original connection remains"),
        ENDPOINT
    );
}

#[test]
fn gh_connection_persists_only_its_exact_credential_reference() {
    let (_directory, store) = store();
    store
        .register_gh_cli_connection(
            "github",
            "github",
            "https://api.githubcopilot.com/mcp/readonly",
            "GitHub.COM",
            "Anrahya",
        )
        .expect("register exact gh credential reference");
    store
        .register_gh_cli_connection(
            "github",
            "github",
            "https://api.githubcopilot.com/mcp/readonly",
            "github.com",
            "Anrahya",
        )
        .expect("canonical hostname makes registration idempotent");

    let config = store
        .connection_config("github")
        .expect("load credential reference");
    assert_eq!(
        config.auth,
        McpConnectionAuth::GhCli {
            hostname: "github.com".to_owned(),
            account: "Anrahya".to_owned(),
        }
    );
    assert!(matches!(
        store.register_gh_cli_connection(
            "github",
            "github",
            "https://api.githubcopilot.com/mcp/readonly",
            "github.com",
            "DifferentAccount",
        ),
        Err(McpHostError::Conflict(_))
    ));
}

#[test]
fn catalog_publication_is_atomic_when_a_late_tool_insert_fails() {
    let (_directory, store) = store();
    store
        .register_direct_connection("example", "primary", ENDPOINT)
        .expect("register connection");
    let original = snapshot("primary", ENDPOINT, &["old"]);
    store
        .publish_catalog(&original)
        .expect("publish original catalog");
    let connection = Connection::open(store.path()).expect("open test injection connection");
    connection
        .execute_batch(
            "CREATE TRIGGER reject_zeta
             BEFORE INSERT ON mcp_tools
             WHEN NEW.name = 'zeta'
             BEGIN
                SELECT RAISE(ABORT, 'injected catalog failure');
             END;",
        )
        .expect("install failure trigger");

    let replacement = snapshot("primary", ENDPOINT, &["alpha", "zeta"]);
    assert!(matches!(
        store.publish_catalog(&replacement),
        Err(McpHostError::Database(_))
    ));

    let stored = store.load_catalog("primary").expect("load prior catalog");
    assert_eq!(stored, original);
}

#[test]
fn a_catalog_cannot_be_published_under_a_different_registered_endpoint() {
    let (_directory, store) = store();
    store
        .register_direct_connection("example", "primary", ENDPOINT)
        .expect("register connection");

    let error = store
        .publish_catalog(&snapshot(
            "primary",
            "http://127.0.0.1:43128/mcp",
            &["echo"],
        ))
        .expect_err("catalog endpoint must match registration");

    assert!(matches!(error, McpHostError::Invalid(_)));
    assert!(matches!(
        store.load_catalog("primary"),
        Err(McpHostError::NotFound(_))
    ));
}

#[test]
fn alpha_selection_survives_a_store_restart_without_duplicates() {
    let (directory, store) = store();
    store
        .register_direct_connection("example", "primary", ENDPOINT)
        .expect("register connection");
    store
        .publish_catalog(&snapshot("primary", ENDPOINT, &["echo"]))
        .expect("publish catalog");
    store
        .select_alpha_tool(PROFILE, "primary", "echo")
        .expect("select tool");
    store
        .select_alpha_tool(PROFILE, "primary", "echo")
        .expect("repeat exact selection");
    drop(store);

    let reopened = McpCatalogStore::initialize(directory.path().join("host.sqlite3"))
        .expect("reopen Host catalog");
    let selected = reopened.alpha_tools(PROFILE).expect("load Alpha tools");

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].integration_id(), "example");
    assert_eq!(selected[0].connection_id(), "primary");
    assert_eq!(selected[0].endpoint(), ENDPOINT);
    assert_eq!(selected[0].tool().name(), "echo");
}

#[test]
fn multi_tool_selection_is_atomic_when_one_name_is_missing() {
    let (_directory, store) = store();
    store
        .register_direct_connection("example", "primary", ENDPOINT)
        .expect("register connection");
    store
        .publish_catalog(&snapshot("primary", ENDPOINT, &["echo"]))
        .expect("publish catalog");

    let error = store
        .select_alpha_tools(PROFILE, "primary", &["echo", "missing"])
        .expect_err("missing tool rejects whole selection batch");

    assert!(matches!(error, McpHostError::NotFound(_)));
    assert!(
        store
            .alpha_tools(PROFILE)
            .expect("load empty selection")
            .is_empty()
    );
}

#[test]
fn a_removed_selected_tool_fails_closed_instead_of_disappearing() {
    let (_directory, store) = store();
    store
        .register_direct_connection("example", "primary", ENDPOINT)
        .expect("register connection");
    store
        .publish_catalog(&snapshot("primary", ENDPOINT, &["echo"]))
        .expect("publish catalog");
    store
        .select_alpha_tool(PROFILE, "primary", "echo")
        .expect("select tool");
    store
        .publish_catalog(&snapshot("primary", ENDPOINT, &["replacement"]))
        .expect("publish replacement catalog");

    assert!(matches!(
        store.alpha_tools(PROFILE),
        Err(McpHostError::NotFound(_))
    ));
}

#[test]
fn stored_catalog_contents_are_checked_against_their_digest() {
    let (_directory, store) = store();
    store
        .register_direct_connection("example", "primary", ENDPOINT)
        .expect("register connection");
    store
        .publish_catalog(&snapshot("primary", ENDPOINT, &["echo"]))
        .expect("publish catalog");
    store
        .select_alpha_tool(PROFILE, "primary", "echo")
        .expect("select tool");
    Connection::open(store.path())
        .expect("open test mutation connection")
        .execute(
            "UPDATE mcp_tools SET description = 'tampered' WHERE connection_id = 'primary'",
            [],
        )
        .expect("mutate stored catalog");

    assert!(matches!(
        store.load_catalog("primary"),
        Err(McpHostError::Invalid(_))
    ));
    assert!(matches!(
        store.alpha_tools(PROFILE),
        Err(McpHostError::Invalid(_))
    ));
}

#[test]
fn a_newer_host_catalog_schema_is_rejected() {
    let (directory, store) = store();
    let path = store.path().to_owned();
    drop(store);
    Connection::open(&path)
        .expect("open schema mutation connection")
        .pragma_update(None, "user_version", 3_u32)
        .expect("advance schema version");

    assert!(matches!(
        McpCatalogStore::initialize(directory.path().join("host.sqlite3")),
        Err(McpHostError::Invalid(_))
    ));
}

#[test]
fn version_one_catalog_migrates_without_losing_no_auth_state() {
    let (directory, store) = store();
    store
        .register_direct_connection("example", "primary", ENDPOINT)
        .expect("register v2 connection");
    store
        .publish_catalog(&snapshot("primary", ENDPOINT, &["echo"]))
        .expect("publish v2 catalog");
    store
        .select_alpha_tool(PROFILE, "primary", "echo")
        .expect("select v2 tool");
    let path = store.path().to_owned();
    drop(store);

    let connection = Connection::open(&path).expect("open migration fixture");
    connection
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
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
        .expect("migrate schema v1 to v2");
    assert_eq!(
        migrated
            .connection_config("primary")
            .expect("load migrated connection")
            .auth,
        McpConnectionAuth::None
    );
    assert_eq!(
        migrated
            .alpha_tools(PROFILE)
            .expect("load migrated selection")[0]
            .tool()
            .name(),
        "echo"
    );
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

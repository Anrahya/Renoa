use rusqlite::Connection;
use serde_json::json;
use tempfile::tempdir;

use super::{
    AdapterCatalog, MCP_ADAPTER_REVISION, MCP_PROTOCOL_VERSION, McpCatalogSnapshot,
    McpCatalogStore, McpCatalogTool, McpConnectionAuth, McpHostError, McpOAuthRegistration,
    McpRejectedTool, McpToolReference, hex_sha256,
};

const PROFILE: &str = "renoa.coding.alpha.v1";
const ENDPOINT: &str = "http://127.0.0.1:43127/mcp";

mod catalog_compatibility;
mod migrations;
mod replacement;

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

#[test]
fn a_supported_legacy_protocol_is_preserved_in_the_catalog() {
    let snapshot = McpCatalogSnapshot::from_adapter(
        "legacy",
        AdapterCatalog {
            endpoint: ENDPOINT.to_owned(),
            protocol_version: "2025-11-25".to_owned(),
            adapter_revision: MCP_ADAPTER_REVISION.to_owned(),
            tools: vec![tool("search")],
            rejected_tools: Vec::new(),
        },
    )
    .expect("supported legacy catalog");

    assert_eq!(snapshot.protocol_version(), "2025-11-25");
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
fn oauth_reference_cannot_be_rebound_to_another_endpoint() {
    let (_directory, store) = store();
    let auth = McpConnectionAuth::oauth("oauth", ENDPOINT, McpOAuthRegistration::dynamic())
        .expect("OAuth reference");
    store
        .register_connection(
            "oauth-integration",
            "oauth",
            ENDPOINT,
            &super::McpRequestHeaders::default(),
            &auth,
        )
        .expect("register OAuth connection");
    let catalog = snapshot("oauth", ENDPOINT, &["search"]);
    store
        .publish_and_enable_connection(PROFILE, &catalog)
        .expect("publish OAuth catalog");
    let reference = McpToolReference::new("oauth", catalog.digest(), "search")
        .expect("exact OAuth tool reference");
    let wrong = McpConnectionAuth::oauth(
        "oauth",
        "https://other.example/mcp",
        McpOAuthRegistration::dynamic(),
    )
    .expect("different endpoint reference")
    .stored_credential_id()
    .expect("OAuth has credential reference")
    .to_owned();
    Connection::open(store.path())
        .expect("open test mutation connection")
        .execute(
            "UPDATE mcp_connections SET auth_credential_id = ?1 WHERE connection_id = 'oauth'",
            [&wrong],
        )
        .expect("mutate stored OAuth reference");

    assert!(matches!(
        store.resolve_alpha_tools(PROFILE, &[reference]),
        Err(McpHostError::Invalid(_))
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
fn registered_plugin_catalog_publication_rolls_back_catalog_and_attachment() {
    let (_directory, store) = store();
    store
        .register_connection(
            "plugin.integration",
            "plugin",
            ENDPOINT,
            &super::McpRequestHeaders::default(),
            &McpConnectionAuth::None,
        )
        .expect("register plugin connection before discovery");
    let connection = Connection::open(store.path()).expect("open test injection connection");
    connection
        .execute_batch(
            "CREATE TRIGGER reject_zeta_plugin
             BEFORE INSERT ON mcp_tools
             WHEN NEW.name = 'zeta'
             BEGIN
                SELECT RAISE(ABORT, 'injected plugin publication failure');
             END;",
        )
        .expect("install failure trigger");
    let snapshot = snapshot("plugin", ENDPOINT, &["alpha", "zeta"]);

    assert!(
        store
            .publish_and_enable_connection(PROFILE, &snapshot)
            .is_err()
    );
    assert!(store.connection_config("plugin").is_ok());
    assert!(
        store
            .alpha_connection_ids(PROFILE)
            .expect("load Alpha attachments")
            .is_empty()
    );
    let persisted_integrations: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM mcp_integrations WHERE integration_id = 'plugin.integration'",
            [],
            |row| row.get(0),
        )
        .expect("count rolled-back integration rows");
    assert_eq!(persisted_integrations, 1);
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
fn alpha_connection_survives_a_store_restart_and_exposes_its_complete_catalog() {
    let (directory, store) = store();
    store
        .register_direct_connection("example", "primary", ENDPOINT)
        .expect("register connection");
    store
        .publish_catalog(&snapshot("primary", ENDPOINT, &["echo", "unused"]))
        .expect("publish catalog");
    store
        .enable_alpha_connection(PROFILE, "primary")
        .expect("enable connection");
    store
        .enable_alpha_connection(PROFILE, "primary")
        .expect("repeat exact enable");
    drop(store);

    let reopened = McpCatalogStore::initialize(directory.path().join("host.sqlite3"))
        .expect("reopen Host catalog");
    let tools = reopened
        .alpha_tool_summaries(PROFILE)
        .expect("load searchable tools");

    assert_eq!(
        reopened
            .alpha_connection_ids(PROFILE)
            .expect("load enabled Alpha connections"),
        ["primary"]
    );
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].integration_id, "example");
    assert_eq!(tools[0].connection_id, "primary");
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[1].name, "unused");
}

#[test]
fn connection_status_and_disconnect_keep_catalogs_but_remove_alpha_access() {
    let (_directory, store) = store();
    store
        .register_direct_connection("example", "primary", ENDPOINT)
        .expect("register direct connection");
    let catalog = snapshot("primary", ENDPOINT, &["echo"]);
    store
        .publish_and_enable_connection(PROFILE, &catalog)
        .expect("publish and enable direct catalog");
    let reference = McpToolReference::new("primary", catalog.digest(), "echo")
        .expect("exact enabled reference");
    let oauth_endpoint = "https://oauth.example/mcp";
    store
        .register_connection(
            "oauth-integration",
            "oauth",
            oauth_endpoint,
            &super::McpRequestHeaders::default(),
            &McpConnectionAuth::oauth("oauth", oauth_endpoint, McpOAuthRegistration::dynamic())
                .expect("OAuth reference"),
        )
        .expect("register OAuth connection without a catalog");

    let before = serde_json::to_value(
        store
            .alpha_connection_statuses(PROFILE)
            .expect("list connection states"),
    )
    .expect("encode connection states");
    let connections = before.as_array().expect("connection state array");
    let primary = connections
        .iter()
        .find(|connection| connection["connection"] == "primary")
        .expect("direct connection state");
    assert_eq!(primary["auth"], "none");
    assert_eq!(primary["registered"], true);
    assert_eq!(primary["catalog_loaded"], true);
    assert_eq!(primary["enabled_for_alpha"], true);
    assert_eq!(primary["tools"], 1);
    let oauth = connections
        .iter()
        .find(|connection| connection["connection"] == "oauth")
        .expect("OAuth connection state");
    assert_eq!(oauth["auth"], "oauth");
    assert_eq!(oauth["registered"], true);
    assert_eq!(oauth["catalog_loaded"], false);
    assert_eq!(oauth["enabled_for_alpha"], false);
    assert_eq!(oauth["tools"], 0);
    assert!(!before.to_string().contains("credential_id"));
    assert!(before[0].get("endpoint").is_none());
    assert!(before[0].get("catalog_digest").is_none());

    assert!(
        store
            .disable_alpha_connection(PROFILE, "primary")
            .expect("disconnect Alpha while retaining the catalog")
    );
    assert!(
        store
            .disable_alpha_connection(PROFILE, "primary")
            .expect("repeating disconnect is idempotent")
    );
    assert!(
        store
            .alpha_tool_summaries(PROFILE)
            .expect("read tools after disconnect")
            .is_empty()
    );
    assert!(matches!(
        store.resolve_alpha_tools(PROFILE, &[reference]),
        Err(McpHostError::NotFound(_))
    ));
    assert_eq!(
        store
            .load_catalog("primary")
            .expect("catalog survives disconnect"),
        catalog
    );
    let after = serde_json::to_value(
        store
            .alpha_connection_statuses(PROFILE)
            .expect("list states after disconnect"),
    )
    .expect("encode states after disconnect");
    let primary = after
        .as_array()
        .expect("connection state array")
        .iter()
        .find(|connection| connection["connection"] == "primary")
        .expect("disconnected direct connection state");
    assert_eq!(primary["catalog_loaded"], true);
    assert_eq!(primary["enabled_for_alpha"], false);
}

#[test]
fn enabling_a_connection_requires_a_complete_catalog() {
    let (_directory, store) = store();
    store
        .register_direct_connection("example", "primary", ENDPOINT)
        .expect("register connection");

    let error = store
        .enable_alpha_connection(PROFILE, "primary")
        .expect_err("missing catalog rejects enable");

    assert!(matches!(error, McpHostError::NotFound(_)));
    assert!(
        store
            .alpha_connection_ids(PROFILE)
            .expect("load empty profile")
            .is_empty()
    );
}

#[test]
fn catalog_refresh_is_hot_and_old_references_fail_closed() {
    let (_directory, store) = store();
    store
        .register_direct_connection("example", "primary", ENDPOINT)
        .expect("register connection");
    let original = snapshot("primary", ENDPOINT, &["echo"]);
    store.publish_catalog(&original).expect("publish catalog");
    store
        .enable_alpha_connection(PROFILE, "primary")
        .expect("enable connection");
    let old_reference =
        McpToolReference::new("primary", original.digest(), "echo").expect("old exact reference");
    store
        .publish_catalog(&snapshot("primary", ENDPOINT, &["replacement"]))
        .expect("publish replacement catalog");

    assert_eq!(
        store
            .alpha_tool_summaries(PROFILE)
            .expect("load refreshed search catalog")[0]
            .name,
        "replacement"
    );
    assert!(matches!(
        store.resolve_alpha_tools(PROFILE, &[old_reference]),
        Err(McpHostError::Conflict(_))
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
    let catalog = store.load_catalog("primary").expect("load exact catalog");
    store
        .enable_alpha_connection(PROFILE, "primary")
        .expect("enable connection");
    let reference =
        McpToolReference::new("primary", catalog.digest(), "echo").expect("exact reference");
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
        store.resolve_alpha_tools(PROFILE, &[reference]),
        Err(McpHostError::Invalid(_))
    ));
}

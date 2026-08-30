use serde::Serialize;

use super::{ENDPOINT, PROFILE, store, tool};
use crate::mcp::{
    AdapterCatalog, MCP_PROTOCOL_VERSION, McpCatalogSnapshot, McpRequestHeaders, McpToolReference,
    hex_sha256,
};

#[derive(Serialize)]
struct HistoricalDigest<'a> {
    endpoint: &'a str,
    protocol_version: &'a str,
    adapter_revision: &'a str,
    tools: &'a [crate::mcp::McpCatalogTool],
    rejected_tools: &'a [crate::mcp::McpRejectedTool],
}

#[test]
fn every_released_catalog_revision_remains_resolvable_after_upgrade() {
    let (_directory, store) = store();
    for revision in [
        "mcp-client-node-v0.1.0",
        "mcp-client-node-v0.2.0",
        "mcp-client-node-v0.4.0",
        "mcp-client-node-v0.5.0",
    ] {
        let connection = revision.replace('.', "-");
        store
            .register_direct_connection("fixture", &connection, ENDPOINT)
            .expect("register historical connection");
        let snapshot = McpCatalogSnapshot::from_stored_with_headers(
            &connection,
            McpRequestHeaders::default(),
            AdapterCatalog {
                endpoint: ENDPOINT.to_owned(),
                protocol_version: MCP_PROTOCOL_VERSION.to_owned(),
                adapter_revision: revision.to_owned(),
                tools: vec![tool("search")],
                rejected_tools: Vec::new(),
            },
        )
        .expect("validate released historical catalog");
        store
            .publish_and_enable_connection(PROFILE, &snapshot)
            .expect("store historical catalog");
        let reference = McpToolReference::new(&connection, snapshot.digest(), "search")
            .expect("historical tool reference");

        let resolved = store
            .resolve_profile_tools(PROFILE, &[reference])
            .expect("resolve historical catalog with the current runtime");
        assert_eq!(resolved[0].adapter_revision(), revision);
    }
}

#[test]
fn early_catalog_digest_keeps_its_original_headerless_shape() {
    let snapshot = McpCatalogSnapshot::from_stored_with_headers(
        "legacy",
        McpRequestHeaders::default(),
        AdapterCatalog {
            endpoint: ENDPOINT.to_owned(),
            protocol_version: MCP_PROTOCOL_VERSION.to_owned(),
            adapter_revision: "mcp-client-node-v0.2.0".to_owned(),
            tools: vec![tool("search")],
            rejected_tools: Vec::new(),
        },
    )
    .expect("validate historical catalog");
    let tools = [tool("search")];

    let expected = hex_sha256(
        &serde_json::to_vec(&HistoricalDigest {
            endpoint: ENDPOINT,
            protocol_version: MCP_PROTOCOL_VERSION,
            adapter_revision: "mcp-client-node-v0.2.0",
            tools: &tools,
            rejected_tools: &[],
        })
        .expect("encode historical digest"),
    );

    assert_eq!(snapshot.digest(), expected);
}

#[test]
fn an_unreleased_catalog_revision_is_rejected() {
    let error = McpCatalogSnapshot::from_stored_with_headers(
        "unknown",
        McpRequestHeaders::default(),
        AdapterCatalog {
            endpoint: ENDPOINT.to_owned(),
            protocol_version: MCP_PROTOCOL_VERSION.to_owned(),
            adapter_revision: "mcp-client-node-v0.3.0".to_owned(),
            tools: Vec::new(),
            rejected_tools: Vec::new(),
        },
    )
    .expect_err("unknown historical revision must fail closed");

    assert!(error.to_string().contains("unsupported adapter revision"));
}

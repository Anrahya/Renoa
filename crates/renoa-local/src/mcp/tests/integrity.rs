use rusqlite::Connection;

use super::{ENDPOINT, PROFILE, snapshot, store};
use crate::mcp::{McpHostError, McpToolReference};

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

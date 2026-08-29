use super::{ENDPOINT, PROFILE, snapshot, store};
use crate::mcp::{McpConnectionAuth, McpHostError, McpOAuthRegistration, McpRequestHeaders};

#[test]
fn explicit_connection_replacement_drops_stale_tools_and_is_idempotent() {
    let (_directory, store) = store();
    store
        .register_direct_connection("stable-integration", "drive", ENDPOINT)
        .expect("register original no-auth connection");
    let original = snapshot("drive", ENDPOINT, &["search"]);
    store
        .publish_and_enable_connection(PROFILE, &original)
        .expect("publish original catalog");
    let replacement = McpConnectionAuth::oauth(
        "drive",
        ENDPOINT,
        McpOAuthRegistration::pre_registered("drive.client").expect("credential reference"),
    )
    .expect("replacement OAuth reference");

    store
        .replace_connection(
            "stable-integration",
            "drive",
            ENDPOINT,
            &McpRequestHeaders::default(),
            &replacement,
        )
        .expect("replace bad connection configuration");
    assert_eq!(
        store
            .connection_config("drive")
            .expect("load replacement")
            .auth,
        replacement
    );
    assert!(matches!(
        store.load_catalog("drive"),
        Err(McpHostError::NotFound(_))
    ));
    assert!(
        store
            .alpha_tool_summaries(PROFILE)
            .expect("load detached profile")
            .is_empty()
    );

    let refreshed = snapshot("drive", ENDPOINT, &["read", "search"]);
    store
        .publish_and_enable_connection(PROFILE, &refreshed)
        .expect("publish replacement catalog");
    store
        .replace_connection(
            "stable-integration",
            "drive",
            ENDPOINT,
            &McpRequestHeaders::default(),
            &replacement,
        )
        .expect("repeat identical replacement");
    assert_eq!(
        store
            .alpha_tool_summaries(PROFILE)
            .expect("identical replacement preserves attachment")
            .len(),
        2
    );
}

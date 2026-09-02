use super::{ENDPOINT, PROFILE, snapshot, store};
use crate::mcp::{
    McpConnectionAuth, McpConnectionCandidate, McpOAuthRegistration, McpRequestHeaders,
};

#[test]
fn explicit_connection_replacement_commits_config_catalog_and_attachment_together() {
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

    let candidate = McpConnectionCandidate::new(
        "stable-integration".to_owned(),
        "drive".to_owned(),
        ENDPOINT.to_owned(),
        McpRequestHeaders::default(),
        replacement.clone(),
    )
    .expect("valid replacement candidate");
    let refreshed = snapshot("drive", ENDPOINT, &["read", "search"]);
    store
        .commit_connection(PROFILE, &candidate, &refreshed, true)
        .expect("commit complete replacement");
    assert_eq!(
        store
            .connection_config("drive")
            .expect("load replacement")
            .auth,
        replacement
    );
    assert_eq!(
        store
            .load_catalog("drive")
            .expect("load replacement catalog"),
        refreshed
    );
    assert_eq!(
        store
            .profile_tool_summaries(PROFILE)
            .expect("load preserved profile attachment")
            .len(),
        2
    );
    store
        .commit_connection(PROFILE, &candidate, &refreshed, true)
        .expect("repeat identical replacement");
    assert_eq!(
        store
            .profile_tool_summaries(PROFILE)
            .expect("identical replacement preserves attachment")
            .len(),
        2
    );
}

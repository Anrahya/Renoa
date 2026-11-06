use std::collections::BTreeSet;

use renoa_control::HostMethod::{self, Get, Post, Put};

use super::{Operation, resolve};

/// Exhaustive on purpose: a new operation must be named here and routed below.
fn name(operation: &Operation) -> &'static str {
    match operation {
        Operation::Observe => "observe",
        Operation::AgentOptions => "agent_options",
        Operation::AgentDefinition(_) => "agent_definition",
        Operation::AgentCreate => "agent_create",
        Operation::HeartbeatRead(_) => "heartbeat_read",
        Operation::HeartbeatSet(_) => "heartbeat_set",
        Operation::AppsList(_) => "apps_list",
        Operation::AppSet(..) => "app_set",
        Operation::ProfileRead => "profile_read",
        Operation::ProfileReplace => "profile_replace",
        Operation::MemoryRead => "memory_read",
        Operation::DiscordStatus => "discord_status",
        Operation::DiscordInspect => "discord_inspect",
        Operation::DiscordConnect => "discord_connect",
        Operation::DiscordChannels => "discord_channels",
        Operation::DiscordBind => "discord_bind",
        Operation::AutomationEnabled(_) => "automation_enabled",
        Operation::AutomationRun(_) => "automation_run",
        Operation::MainAgentSet => "main_agent_set",
        Operation::FeedList(None) => "feed_list",
        Operation::FeedList(Some(_)) => "feed_list_before",
        Operation::FeedSee => "feed_see",
        Operation::FeedDismiss(_) => "feed_dismiss",
        Operation::UploadStart(_) => "upload_start",
        Operation::UploadPart(..) => "upload_part",
        Operation::ShareDescribe(_) => "share_describe",
        Operation::SharePart(..) => "share_part",
    }
}

fn status(method: HostMethod, path: &str) -> u16 {
    resolve(method, path).expect_err("no operation").status
}

/// One route to each operation, with the operation's name.
fn routes() -> Vec<(HostMethod, String, &'static str)> {
    let id = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";
    vec![
        (Get, "/v1/host".to_owned(), "observe"),
        (Get, "/v1/host/agents/options".to_owned(), "agent_options"),
        (Get, format!("/v1/host/agents/{id}"), "agent_definition"),
        (Post, "/v1/host/agents".to_owned(), "agent_create"),
        (
            Get,
            format!("/v1/host/agents/{id}/heartbeat"),
            "heartbeat_read",
        ),
        (
            Put,
            format!("/v1/host/agents/{id}/heartbeat"),
            "heartbeat_set",
        ),
        (Get, format!("/v1/host/agents/{id}/apps"), "apps_list"),
        (
            Post,
            format!("/v1/host/agents/{id}/apps/agentmail/enabled"),
            "app_set",
        ),
        (Get, "/v1/host/profile".to_owned(), "profile_read"),
        (Put, "/v1/host/profile".to_owned(), "profile_replace"),
        (Get, "/v1/host/memory".to_owned(), "memory_read"),
        (Get, "/v1/host/discord".to_owned(), "discord_status"),
        (
            Post,
            "/v1/host/discord/inspection".to_owned(),
            "discord_inspect",
        ),
        (
            Post,
            "/v1/host/discord/connection".to_owned(),
            "discord_connect",
        ),
        (
            Get,
            "/v1/host/discord/channels".to_owned(),
            "discord_channels",
        ),
        (Post, "/v1/host/discord/bindings".to_owned(), "discord_bind"),
        (
            Post,
            format!("/v1/host/automations/{id}/enabled"),
            "automation_enabled",
        ),
        (
            Post,
            format!("/v1/host/automations/{id}/run"),
            "automation_run",
        ),
        (Post, "/v1/host/main-agent".to_owned(), "main_agent_set"),
        (Get, "/v1/host/feed".to_owned(), "feed_list"),
        (
            Get,
            "/v1/host/feed/before/120".to_owned(),
            "feed_list_before",
        ),
        (Post, "/v1/host/feed/seen".to_owned(), "feed_see"),
        (
            Post,
            format!("/v1/host/feed/items/{id}/dismissal"),
            "feed_dismiss",
        ),
        (Put, format!("/v1/host/uploads/{id}"), "upload_start"),
        (Put, format!("/v1/host/uploads/{id}/parts/0"), "upload_part"),
        (Get, format!("/v1/host/shares/{id}"), "share_describe"),
        (Get, format!("/v1/host/shares/{id}/parts/0"), "share_part"),
    ]
}

#[test]
fn every_operation_has_a_route_and_every_route_one_operation() {
    let mut reached = BTreeSet::new();
    for (method, path, expected) in &routes() {
        let operation = resolve(*method, path).expect("route resolves");
        assert_eq!(name(&operation), *expected, "{method:?} {path}");
        reached.insert(*expected);
    }
    let every_operation: BTreeSet<_> = [
        "observe",
        "agent_options",
        "agent_definition",
        "agent_create",
        "heartbeat_read",
        "heartbeat_set",
        "apps_list",
        "app_set",
        "profile_read",
        "profile_replace",
        "memory_read",
        "discord_status",
        "discord_inspect",
        "discord_connect",
        "discord_channels",
        "discord_bind",
        "automation_enabled",
        "automation_run",
        "main_agent_set",
        "feed_list",
        "feed_list_before",
        "feed_see",
        "feed_dismiss",
        "upload_start",
        "upload_part",
        "share_describe",
        "share_part",
    ]
    .into_iter()
    .collect();
    assert_eq!(reached, every_operation);
}

#[test]
fn a_known_path_with_another_method_is_refused_and_an_unknown_path_is_not_found() {
    assert_eq!(status(Post, "/v1/host"), 405);
    assert_eq!(status(Get, "/v1/host/agents"), 405);
    assert_eq!(status(Put, "/v1/host/agents/options"), 405);
    assert_eq!(status(Get, "/v1/host/discord/inspection"), 405);
    assert_eq!(status(Get, "/v1/host/main-agent"), 405);
    assert_eq!(status(Post, "/v1/host/feed"), 405);
    assert_eq!(status(Get, "/v1/host/feed/seen"), 405);
    assert_eq!(status(Get, "/v1/host/automations/a/run"), 405);
    assert_eq!(status(Get, "/v1/host/uploads/a"), 405);
    assert_eq!(status(Post, "/v1/host/uploads/a/parts/0"), 405);
    assert_eq!(status(Put, "/v1/host/shares/a"), 405);
    assert_eq!(status(Post, "/v1/host/shares/a/parts/0"), 405);
    for path in [
        "/v1/hostile",
        "/v1/host/",
        "/v1/host/unknown",
        "/v1/host/agents/a/b",
        "/v1/host/automations/a/disabled",
        "/v1/host//agents",
        "/v1/identity/session",
        "/",
    ] {
        assert_eq!(status(Get, path), 404, "{path}");
    }
}

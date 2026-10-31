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
        Operation::ProfileRead => "profile_read",
        Operation::ProfileReplace => "profile_replace",
        Operation::DiscordStatus => "discord_status",
        Operation::DiscordInspect => "discord_inspect",
        Operation::DiscordConnect => "discord_connect",
        Operation::DiscordChannels => "discord_channels",
        Operation::DiscordBind => "discord_bind",
        Operation::AutomationEnabled(_) => "automation_enabled",
        Operation::MainAgentSet => "main_agent_set",
    }
}

fn status(method: HostMethod, path: &str) -> u16 {
    resolve(method, path).expect_err("no operation").status
}

#[test]
fn every_operation_has_a_route_and_every_route_one_operation() {
    let id = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";
    let routes = [
        (Get, "/v1/host".to_owned(), "observe"),
        (Get, "/v1/host/agents/options".to_owned(), "agent_options"),
        (Get, format!("/v1/host/agents/{id}"), "agent_definition"),
        (Post, "/v1/host/agents".to_owned(), "agent_create"),
        (Get, "/v1/host/profile".to_owned(), "profile_read"),
        (Put, "/v1/host/profile".to_owned(), "profile_replace"),
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
        (Post, "/v1/host/main-agent".to_owned(), "main_agent_set"),
    ];
    let mut reached = BTreeSet::new();
    for (method, path, expected) in &routes {
        let operation = resolve(*method, path).expect("route resolves");
        assert_eq!(name(&operation), *expected, "{method:?} {path}");
        reached.insert(*expected);
    }
    let every_operation: BTreeSet<_> = [
        "observe",
        "agent_options",
        "agent_definition",
        "agent_create",
        "profile_read",
        "profile_replace",
        "discord_status",
        "discord_inspect",
        "discord_connect",
        "discord_channels",
        "discord_bind",
        "automation_enabled",
        "main_agent_set",
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

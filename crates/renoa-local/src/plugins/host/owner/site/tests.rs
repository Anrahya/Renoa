//! `site_manage` discovery through `HostPlugins` and execution through
//! `plugin_search` / `tool_execute` semantics.

use std::{fs, path::PathBuf};

use renoa_agent::{ContentBlock, ToolCall, ToolResult};
use renoa_home::RenoaHome;
use renoa_kernel::{AgentId, SessionId};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::super::{OwnerTurn, bindings};
use crate::{
    host::catalog,
    plugins::host::{HostPluginId, HostPlugins, state},
    sites::{SiteEntry, SiteError, Sites},
};

struct Fixture {
    _directory: tempfile::TempDir,
    home: RenoaHome,
    database: PathBuf,
    workspace: PathBuf,
    agent: AgentId,
    person: Uuid,
    site_origin: Option<String>,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path().canonicalize().expect("canonical root");
    let home = RenoaHome::at(root.join("home")).expect("home");
    home.initialize().expect("init home");
    let database = home.host_database();
    catalog::initialize(&database).expect("catalog");
    let agent_uuid = Uuid::new_v4();
    crate::test_agents::insert_agent(&database, &agent_uuid.to_string());
    let agent = AgentId::from_uuid(agent_uuid);
    let workspace = root.join("workspace");
    fs::create_dir(&workspace).expect("workspace");
    Fixture {
        _directory: directory,
        home,
        database,
        workspace,
        agent,
        person: Uuid::new_v4(),
        site_origin: Some(ORIGIN.to_owned()),
    }
}

const ORIGIN: &str = "https://renoa-sites.test:8443";

impl Fixture {
    fn plugins(&self) -> HostPlugins {
        let turn = OwnerTurn {
            database: self.database.clone(),
            agent: self.agent,
            session: SessionId::new(),
            command: None,
            workspace: self.workspace.clone(),
            principal: Some(self.person),
            site_origin: self.site_origin.clone(),
        };
        HostPlugins::new(self.database.clone(), self.agent, bindings(turn).into())
            .expect("host plugins")
    }

    async fn call(&self, reference: &str, arguments: serde_json::Value) -> ToolResult {
        let plugins = self.plugins();
        let tool = plugins.resolve(reference).expect("resolved site_manage");
        renoa_agent::invoke_tool(
            Some(tool.as_ref()),
            ToolCall {
                id: "call".to_owned(),
                name: tool.spec().name.clone(),
                arguments,
                namespace: None,
                thought_signature: None,
            },
            CancellationToken::new(),
            None,
        )
        .await
        .expect("definite result")
    }

    fn text(output: &ToolResult) -> String {
        match output.content.as_slice() {
            [ContentBlock::Text { text, .. }] => text.clone(),
            other => panic!("expected one text block, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn host_plugins_search_finds_site_manage_and_runs_publish_list_unpublish() {
    let fixture = fixture();
    let dist = fixture.workspace.join("dist");
    fs::create_dir(&dist).expect("dist");
    fs::write(dist.join("index.html"), "<h1>Hello</h1>").expect("index");

    let plugins = fixture.plugins();
    let matches = plugins.matches(None, "static site", true).expect("search");
    let card = matches
        .iter()
        .find(|m| m.name == super::MANAGE)
        .expect("site_manage should be discoverable under renoa.owner");
    assert!(card.reference.starts_with("host:"), "{}", card.reference);

    let publish = fixture
        .call(
            &card.reference,
            json!({ "action": "publish", "name": "demo", "folder": "dist" }),
        )
        .await;
    assert!(!publish.is_error, "{publish:?}");
    let publish_text = Fixture::text(&publish);
    assert!(
        publish_text.contains(&format!("[demo]({ORIGIN}/sites/demo/)")),
        "{publish_text}"
    );

    let sites = Sites::open(&fixture.home);
    assert!(matches!(
        sites.entry("demo", "").await,
        Ok(SiteEntry::File(index)) if index.ends_with("index.html")
    ));

    let listed = fixture
        .call(&card.reference, json!({ "action": "list" }))
        .await;
    assert!(!listed.is_error, "{listed:?}");
    let listed_text = Fixture::text(&listed);
    assert!(
        listed_text.contains(&format!("[demo]({ORIGIN}/sites/demo/)")),
        "{listed_text}"
    );

    let unpublished = fixture
        .call(
            &card.reference,
            json!({ "action": "unpublish", "name": "demo" }),
        )
        .await;
    assert!(!unpublished.is_error, "{unpublished:?}");
    assert!(matches!(
        sites.entry("demo", "").await,
        Err(SiteError::NotFound)
    ));

    let empty_list = fixture
        .call(&card.reference, json!({ "action": "list" }))
        .await;
    assert_eq!(Fixture::text(&empty_list), "No static sites are published.");

    state::change(
        &fixture.database,
        fixture.agent,
        HostPluginId::Owner,
        false,
        "op-disable",
    )
    .expect("disable owner");
    assert!(
        fixture.plugins().resolve(&card.reference).is_err(),
        "disabled renoa.owner must block site_manage"
    );
}

#[tokio::test]
async fn site_manage_refuses_bad_arguments_without_residue() {
    let fixture = fixture();
    let plugins = fixture.plugins();
    let reference = plugins
        .matches(None, "static site", true)
        .expect("search")
        .into_iter()
        .find(|m| m.name == super::MANAGE)
        .expect("site_manage")
        .reference;

    for bad in [
        json!({ "action": "publish", "name": "demo" }),
        json!({ "action": "publish", "folder": "dist" }),
        json!({ "action": "publish", "name": "-bad-", "folder": "dist" }),
        json!({ "action": "publish", "name": "demo", "folder": "../escape" }),
        json!({ "action": "list", "name": "demo" }),
        json!({ "action": "unpublish" }),
        json!({ "action": "unpublish", "name": "demo", "folder": "dist" }),
        json!({ "action": "list", "extra": true }),
    ] {
        let out = fixture.call(&reference, bad.clone()).await;
        assert!(out.is_error, "expected refusal for {bad}");
    }
    assert!(fs::read_dir(fixture.home.sites()).map_or(true, |mut d| d.next().is_none()));
}

#[tokio::test]
async fn without_a_site_gateway_site_manage_refuses_before_copying_anything() {
    let mut fixture = fixture();
    fixture.site_origin = None;
    let dist = fixture.workspace.join("dist");
    fs::create_dir(&dist).expect("dist");
    fs::write(dist.join("index.html"), "<h1>Hello</h1>").expect("index");
    let reference = fixture
        .plugins()
        .matches(None, "static site", true)
        .expect("search")
        .into_iter()
        .find(|m| m.name == super::MANAGE)
        .expect("site_manage")
        .reference;

    let out = fixture
        .call(
            &reference,
            json!({ "action": "publish", "name": "demo", "folder": "dist" }),
        )
        .await;
    assert!(out.is_error, "{out:?}");
    assert!(
        Fixture::text(&out).contains("site gateway"),
        "{}",
        Fixture::text(&out)
    );
    assert!(fs::read_dir(fixture.home.sites()).map_or(true, |mut d| d.next().is_none()));
    assert!(matches!(
        Sites::open(&fixture.home).entry("demo", "").await,
        Err(SiteError::NotFound)
    ));
}

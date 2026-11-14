//! `app_manage` discovery through `HostPlugins` and execution through
//! `plugin_search` / tool invocation semantics.

use std::{cell::RefCell, fs, path::PathBuf};

use renoa_agent::{ContentBlock, ToolCall, ToolResult};
use renoa_home::RenoaHome;
use renoa_kernel::{AgentId, SessionId};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::super::{OwnerTurn, bindings};
use crate::{
    apps::{AppState, Apps, stop_unit_sync},
    host::catalog,
    plugins::host::{HostPluginId, HostPlugins, state},
};

const ORIGIN: &str = "https://renoa-sites.test:8445";

const PYTHON_APP: &str = r#"import http.server, os
PORT = int(os.environ["PORT"])
BASE = os.environ.get("BASE_PATH", "")
print(f"app boot {PORT} {BASE}", flush=True)
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(f"ok:{BASE}".encode())
    def log_message(self, format, *args):
        pass
http.server.ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
"#;

struct UnitCleanup {
    database: PathBuf,
    names: RefCell<Vec<String>>,
}

impl UnitCleanup {
    fn track(&self, name: &str) {
        self.names.borrow_mut().push(name.to_owned());
    }
}

impl Drop for UnitCleanup {
    fn drop(&mut self) {
        for name in self.names.borrow().iter() {
            stop_unit_sync(&self.database, name);
        }
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    home: RenoaHome,
    database: PathBuf,
    workspace: PathBuf,
    agent: AgentId,
    person: Uuid,
    site_origin: Option<String>,
    cleanup: UnitCleanup,
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
        database: database.clone(),
        workspace,
        agent,
        person: Uuid::new_v4(),
        site_origin: Some(ORIGIN.to_owned()),
        cleanup: UnitCleanup {
            database,
            names: RefCell::new(Vec::new()),
        },
    }
}

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
        let tool = plugins.resolve(reference).expect("resolved app_manage");
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
async fn host_plugins_search_finds_app_manage_and_runs_full_lifecycle() {
    let f = fixture();
    let app_dir = f.workspace.join("webapp");
    fs::create_dir(&app_dir).expect("webapp");
    fs::write(app_dir.join("server.py"), PYTHON_APP).expect("server.py");
    f.cleanup.track("demo");

    let plugins = f.plugins();
    let matches = plugins.matches(None, "live web app", true).expect("search");
    let card = matches
        .iter()
        .find(|m| m.name == super::MANAGE)
        .expect("app_manage should be discoverable under renoa.owner");
    assert!(card.reference.starts_with("host:"), "{}", card.reference);

    let started = f
        .call(
            &card.reference,
            json!({
                "action": "start",
                "name": "demo",
                "folder": "webapp",
                "command": "python3 server.py"
            }),
        )
        .await;
    assert!(!started.is_error, "{started:?}");
    let start_text = Fixture::text(&started);
    assert!(
        start_text.contains(&format!("[demo]({ORIGIN}/apps/demo/)")),
        "{start_text}"
    );
    assert!(start_text.contains("state=running"), "{start_text}");

    let status_one = f
        .call(
            &card.reference,
            json!({ "action": "status", "name": "demo" }),
        )
        .await;
    assert!(!status_one.is_error, "{status_one:?}");
    assert!(Fixture::text(&status_one).contains("state=running"));

    let status_all = f.call(&card.reference, json!({ "action": "status" })).await;
    assert!(!status_all.is_error, "{status_all:?}");
    assert!(Fixture::text(&status_all).contains(&format!("[demo]({ORIGIN}/apps/demo/)")));

    let logs = f
        .call(&card.reference, json!({ "action": "logs", "name": "demo" }))
        .await;
    assert!(!logs.is_error, "{logs:?}");
    assert!(Fixture::text(&logs).contains("app boot"));

    let restarted = f
        .call(
            &card.reference,
            json!({ "action": "restart", "name": "demo" }),
        )
        .await;
    assert!(!restarted.is_error, "{restarted:?}");
    assert!(Fixture::text(&restarted).contains("Restarted"));

    let stopped = f
        .call(&card.reference, json!({ "action": "stop", "name": "demo" }))
        .await;
    assert!(!stopped.is_error, "{stopped:?}");
    assert!(Fixture::text(&stopped).contains("state=stopped"));

    let apps = Apps::open(&f.home);
    assert_eq!(
        apps.status_for("demo", Some(f.person))
            .await
            .expect("status")
            .state,
        AppState::Stopped
    );

    state::change(
        &f.database,
        f.agent,
        HostPluginId::Owner,
        false,
        "op-disable",
    )
    .expect("disable owner");
    assert!(
        f.plugins().resolve(&card.reference).is_err(),
        "disabled renoa.owner must block app_manage"
    );
}

#[tokio::test]
async fn app_manage_refuses_bad_arguments_without_residue() {
    let f = fixture();
    let reference = f
        .plugins()
        .matches(None, "live web app", true)
        .expect("search")
        .into_iter()
        .find(|m| m.name == super::MANAGE)
        .expect("app_manage")
        .reference;

    for bad in [
        json!({ "action": "start", "name": "demo", "folder": "app" }),
        json!({ "action": "start", "name": "-bad-", "folder": "app", "command": "python3 x.py" }),
        json!({ "action": "start", "name": "demo", "folder": "../escape", "command": "python3 x.py" }),
        json!({ "action": "start", "name": "demo", "folder": "app", "command": "   " }),
        json!({ "action": "stop" }),
        json!({ "action": "stop", "name": "demo", "folder": "app" }),
        json!({ "action": "restart" }),
        json!({ "action": "restart", "name": "demo", "command": "echo hi" }),
        json!({ "action": "status", "folder": "app" }),
        json!({ "action": "logs" }),
        json!({ "action": "logs", "name": "demo", "command": "echo hi" }),
        json!({ "action": "status", "extra": true }),
    ] {
        let out = f.call(&reference, bad.clone()).await;
        assert!(out.is_error, "expected refusal for {bad}");
    }
    assert!(fs::read_dir(f.home.apps()).map_or(true, |mut d| d.next().is_none()));
}

#[tokio::test]
async fn without_a_site_gateway_app_manage_refuses_before_starting_anything() {
    let mut f = fixture();
    f.site_origin = None;
    let app_dir = f.workspace.join("webapp");
    fs::create_dir(&app_dir).expect("webapp");
    fs::write(app_dir.join("server.py"), PYTHON_APP).expect("server.py");
    f.cleanup.track("demo");

    let reference = f
        .plugins()
        .matches(None, "live web app", true)
        .expect("search")
        .into_iter()
        .find(|m| m.name == super::MANAGE)
        .expect("app_manage")
        .reference;

    let out = f
        .call(
            &reference,
            json!({
                "action": "start",
                "name": "demo",
                "folder": "webapp",
                "command": "python3 server.py"
            }),
        )
        .await;
    assert!(out.is_error, "{out:?}");
    assert!(
        Fixture::text(&out).contains("site gateway"),
        "{}",
        Fixture::text(&out)
    );
    for call in [
        json!({ "action": "status" }),
        json!({ "action": "status", "name": "demo" }),
        json!({ "action": "stop", "name": "demo" }),
        json!({ "action": "restart", "name": "demo" }),
        json!({ "action": "logs", "name": "demo" }),
    ] {
        let refused = f.call(&reference, call.clone()).await;
        assert!(refused.is_error, "expected refusal for {call}");
        assert!(
            Fixture::text(&refused).contains("site gateway"),
            "{}",
            Fixture::text(&refused)
        );
    }
    assert!(fs::read_dir(f.home.apps()).map_or(true, |mut d| d.next().is_none()));
    assert!(!Apps::open(&f.home).exists("demo").expect("exists"));
}

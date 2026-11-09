use gpui_kit::{AppContext as _, Entity, TestAppContext};
use renoa_protocol::TargetRef;
use renoa_rcp_wire::NodeId;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{Profile, its_routines};
use crate::{
    chat::chat_task_id,
    link::{Command, host::Answer},
    sheets::{Host, Loaded, Member, routines::held::Observation},
};

const CHEF: Uuid = Uuid::from_u128(0xc1);
const SCOUT: Uuid = Uuid::from_u128(0x5c);

fn routine(name: &str, agent: Uuid, enabled: bool, next: i64) -> serde_json::Value {
    serde_json::json!({"id": name, "agent_id": agent, "name": name,
        "schedule": {"kind": "cron", "expression": "0 9 * * *", "timezone": "Asia/Kolkata"},
        "enabled": enabled, "revision": 1, "next_due_ms": next})
}

fn observed() -> serde_json::Value {
    let mut reminder = routine("Renew the gym", CHEF, true, 1);
    reminder["plugin"] = "renoa.memory".into();
    serde_json::json!({"automations": [
        routine("Paused", CHEF, false, 5),
        routine("Later", CHEF, true, 30),
        routine("Scout's", SCOUT, true, 2),
        reminder,
        routine("Sooner", CHEF, true, 20),
    ]})
}

#[test]
fn an_agents_page_lists_its_own_routines_on_first_then_soonest() {
    let observed: Observation = serde_json::from_value(observed()).expect("an observation");
    let names: Vec<String> = its_routines(observed.automations, CHEF)
        .into_iter()
        .map(|routine| routine.name)
        .collect();
    assert_eq!(
        names,
        ["Sooner", "Later", "Paused"],
        "another agent's routines and a plugin's own triggers are left out"
    );
}

fn chef() -> Member {
    let node_id = NodeId::from_uuid(Uuid::from_u128(9));
    Member {
        name: "Chef".into(),
        host_id: CHEF,
        node_id,
        target: TargetRef::new("agent:chef"),
        task_id: chat_task_id(node_id, "agent:chef"),
        look: None,
        primary: false,
    }
}

/// Answers the Host calls asked so far; returns the paths asked.
fn serve(
    asked: &mut mpsc::UnboundedReceiver<Command>,
    answer: impl Fn(&str) -> Answer<String>,
    cx: &mut TestAppContext,
) -> Vec<String> {
    let mut paths = Vec::new();
    loop {
        cx.run_until_parked();
        let mut served = false;
        while let Ok(command) = asked.try_recv() {
            if let Command::Host(call) = command {
                served = true;
                paths.push(call.path.clone());
                let _ = call.answered.send(answer(&call.path));
            }
        }
        if !served {
            return paths;
        }
    }
}

fn item(id: u128, kind: &str, agent: Uuid) -> serde_json::Value {
    serde_json::json!({"id": Uuid::from_u128(id), "kind": kind,
        "agent": {"id": agent, "name": "?"}, "title": format!("title {id}"),
        "body": format!("body {id}"), "at_ms": id, "done": id == 4, "subject_id": "s"})
}

/// The Host: Chef's notes start on the feed's second page, past Scout's.
fn host(path: &str) -> Answer<String> {
    let said = match path {
        "/v1/host" => observed(),
        "/v1/host/feed" => serde_json::json!({"today": "", "next_before": 10,
            "items": [item(12, "update", SCOUT), item(11, "update", SCOUT)]}),
        "/v1/host/feed/before/10" => serde_json::json!({"today": "", "next_before": 3, "items": [
            item(9, "update", SCOUT), item(8, "weather", CHEF), item(7, "ask", CHEF),
            item(6, "update", CHEF), item(5, "problem", CHEF), item(4, "result", CHEF),
            item(3, "ability", CHEF),
        ]}),
        path if path.ends_with("/apps") => serde_json::json!({"apps": [
            {"connection": "gmail", "name": "Gmail", "endpoint": "https://gmail.example/mcp", "on": true},
            {"connection": "exa-mcp", "endpoint": "https://mcp.exa.ai/mcp", "on": false},
        ]}),
        _ => serde_json::json!({"id": CHEF, "name": "Chef", "created_at_ms": 7,
            "operational": {"instructions": "Cook."}}),
    };
    Answer::Said(said.to_string())
}

/// What the page shows once read: when the agent came, its routines, its
/// apps and its latest notes, each `None` when unread.
#[allow(clippy::type_complexity)]
fn shown(
    profile: &Entity<Profile>,
    cx: &mut TestAppContext,
) -> Option<(
    i64,
    Option<Vec<String>>,
    Option<Vec<String>>,
    Option<Vec<String>>,
)> {
    profile.read_with(cx, |profile, _| match &profile.loaded {
        Loaded::Ready(held) => Some((
            held.since_ms,
            held.routines.as_ref().map(|routines| {
                routines
                    .iter()
                    .map(|routine| routine.name.clone())
                    .collect()
            }),
            held.apps
                .as_ref()
                .map(|apps| apps.iter().map(|app| app.connection.clone()).collect()),
            held.lately
                .as_ref()
                .map(|lately| lately.iter().map(super::said).collect()),
        )),
        Loaded::Waiting | Loaded::Unread => None,
    })
}

fn open(cx: &mut TestAppContext) -> (Entity<Profile>, mpsc::UnboundedReceiver<Command>) {
    let (commands, asked) = mpsc::unbounded_channel();
    let profile = cx.new(|cx| Profile::new(Host(commands), chef(), None, cx));
    (profile, asked)
}

fn names(names: &[&str]) -> Option<Vec<String>> {
    Some(names.iter().map(|name| (*name).to_owned()).collect())
}

#[gpui_kit::test]
fn an_agents_page_reads_when_it_came_its_routines_its_apps_and_its_latest_notes(
    cx: &mut TestAppContext,
) {
    let (profile, mut asked) = open(cx);
    profile.update(cx, |profile, cx| profile.read(cx));
    let paths = serve(&mut asked, host, cx);
    assert_eq!(
        paths,
        [
            format!("/v1/host/agents/{CHEF}"),
            format!("/v1/host/agents/{CHEF}/apps"),
            "/v1/host".to_owned(),
            "/v1/host/feed".to_owned(),
            "/v1/host/feed/before/10".to_owned(),
        ],
        "one read at a time; the feed is read back until four notes are found"
    );
    assert_eq!(
        shown(&profile, cx),
        Some((
            7,
            names(&["Sooner", "Later", "Paused"]),
            names(&["gmail"]),
            names(&["Asked: body 7", "body 6", "title 5", "title 4"]),
        )),
        "only the apps it has on; its latest notes, done or not, in their own words"
    );
}

#[gpui_kit::test]
fn a_part_that_cannot_be_read_says_so_and_the_rest_still_shows(cx: &mut TestAppContext) {
    let (profile, mut asked) = open(cx);
    serve(
        &mut asked,
        |path| {
            if path.ends_with("/apps") || path.contains("/feed/before/") {
                Answer::Later
            } else {
                host(path)
            }
        },
        cx,
    );
    assert_eq!(
        shown(&profile, cx),
        Some((7, names(&["Sooner", "Later", "Paused"]), None, None))
    );

    let (profile, mut asked) = open(cx);
    serve(
        &mut asked,
        |path| {
            if path.ends_with("/apps") {
                Answer::Missing
            } else {
                host(path)
            }
        },
        cx,
    );
    assert_eq!(
        shown(&profile, cx).and_then(|(_, _, apps, _)| apps),
        Some(Vec::new()),
        "an agent gone while its apps are read has none"
    );
}

#[gpui_kit::test]
fn an_agent_the_host_no_longer_has_is_gone_and_one_unread_can_be_read_again(
    cx: &mut TestAppContext,
) {
    let (profile, mut asked) = open(cx);
    serve(&mut asked, |_| Answer::Missing, cx);
    assert!(profile.read_with(cx, |profile, _| profile.gone));

    let (profile, mut asked) = open(cx);
    serve(&mut asked, |_| Answer::Later, cx);
    assert!(
        profile.read_with(cx, |profile, _| matches!(profile.loaded, Loaded::Unread)
            && !profile.gone)
    );
    profile.update(cx, |profile, cx| profile.read(cx));
    serve(&mut asked, host, cx);
    assert!(shown(&profile, cx).is_some());
}

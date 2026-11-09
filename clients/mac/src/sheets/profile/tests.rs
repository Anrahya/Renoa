use chrono::{TimeZone as _, Utc};
use gpui_kit::{AppContext as _, Entity, TestAppContext};
use renoa_protocol::TargetRef;
use renoa_rcp_wire::NodeId;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{Profile, its_routines, since};
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

#[test]
fn how_long_an_agent_has_been_around_names_the_year_only_when_another() {
    let now = Utc.with_ymd_and_hms(2026, 11, 9, 12, 0, 0).unwrap();
    let at = |year, month, day| {
        Utc.with_ymd_and_hms(year, month, day, 8, 0, 0)
            .unwrap()
            .timestamp_millis()
    };
    assert_eq!(
        since(at(2026, 11, 3), &now).as_deref(),
        Some("With you since 3 November")
    );
    assert_eq!(
        since(at(2025, 12, 24), &now).as_deref(),
        Some("With you since 24 December 2025")
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

fn host(path: &str) -> Answer<String> {
    let said = match path {
        "/v1/host" => observed(),
        "/v1/host/feed" => {
            let item = |id: u128, kind: &str, agent: Uuid| {
                serde_json::json!({"id": Uuid::from_u128(id), "kind": kind,
                    "agent": {"id": agent, "name": "?"}, "title": format!("title {id}"),
                    "body": format!("body {id}"), "at_ms": id, "done": id == 4,
                    "subject_id": "s"})
            };
            serde_json::json!({"today": "", "items": [
                item(9, "update", SCOUT), item(8, "weather", CHEF), item(7, "ask", CHEF),
                item(6, "update", CHEF), item(5, "problem", CHEF), item(4, "result", CHEF),
                item(3, "ability", CHEF),
            ]})
        }
        path if path.ends_with("/apps") => serde_json::json!({"apps": [
            {"connection": "gmail", "name": "Gmail", "endpoint": "https://gmail.example/mcp", "on": true},
            {"connection": "exa-mcp", "endpoint": "https://mcp.exa.ai/mcp", "on": false},
        ]}),
        _ => serde_json::json!({"id": CHEF, "name": "Chef", "created_at_ms": 7,
            "operational": {"instructions": "Cook.", "purpose": "  Plans dinners from the fridge.  "}}),
    };
    Answer::Said(said.to_string())
}

/// What the page shows: its purpose, routines and apps, once read.
#[allow(clippy::type_complexity)]
fn shown(
    profile: &Entity<Profile>,
    cx: &mut TestAppContext,
) -> Option<(Option<String>, i64, Vec<String>, Vec<String>, Vec<String>)> {
    profile.read_with(cx, |profile, _| match &profile.loaded {
        Loaded::Ready(held) => Some((
            held.purpose.clone(),
            held.since_ms,
            held.routines
                .iter()
                .map(|routine| routine.name.clone())
                .collect(),
            held.apps.iter().map(|app| app.connection.clone()).collect(),
            held.lately.iter().map(super::said).collect(),
        )),
        Loaded::Waiting | Loaded::Unread => None,
    })
}

#[gpui_kit::test]
fn an_agents_page_reads_what_it_is_for_its_routines_and_the_apps_it_uses(cx: &mut TestAppContext) {
    let (commands, mut asked) = mpsc::unbounded_channel();
    let profile = cx.new(|cx| Profile::new(Host(commands), chef(), cx));
    let paths = serve(&mut asked, host, cx);
    assert_eq!(
        paths,
        [
            format!("/v1/host/agents/{CHEF}"),
            format!("/v1/host/agents/{CHEF}/apps"),
            "/v1/host".to_owned(),
            "/v1/host/feed".to_owned()
        ]
    );
    assert_eq!(
        shown(&profile, cx),
        Some((
            Some("Plans dinners from the fridge.".to_owned()),
            7,
            vec!["Sooner".to_owned(), "Later".to_owned(), "Paused".to_owned()],
            vec!["gmail".to_owned()],
            ["Asked: body 7", "body 6", "title 5", "title 4"]
                .map(str::to_owned)
                .to_vec(),
        )),
        "only the apps it has on; its latest notes, done or not, in their own words"
    );
}

#[gpui_kit::test]
fn an_agents_page_says_so_when_any_part_cannot_be_read_and_reads_again(cx: &mut TestAppContext) {
    let (commands, mut asked) = mpsc::unbounded_channel();
    let profile = cx.new(|cx| Profile::new(Host(commands), chef(), cx));
    serve(
        &mut asked,
        |path| {
            if path.ends_with("/apps") {
                Answer::Later
            } else {
                host(path)
            }
        },
        cx,
    );
    assert!(profile.read_with(cx, |profile, _| matches!(profile.loaded, Loaded::Unread)));
    profile.update(cx, |profile, cx| profile.read(cx));
    serve(&mut asked, host, cx);
    assert!(shown(&profile, cx).is_some());
}

//! Renders each screen offscreen to a PNG, with a Host that answers as a
//! real one would, so the layouts can be looked at without a display.
//! Run with `RENOA_SCREENS=<dir> cargo test screens -- --ignored`.

use std::{path::PathBuf, sync::Arc};

use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, HeadlessAppContext, px, size,
    test::TestWindowExt as _,
};
use renoa_protocol::{CommandInput, ExecutionEventKind, ExecutionTerminal, TargetRef};
use renoa_rcp_wire::{NodeId, TargetSummary};
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::Chats;
use crate::{
    chat::chat_task_id,
    face::look::{Cheeks, Eyes, HairBack, HairColour, HairFront, Look, Skin, Tile, Wear},
    link::{Command, Update, host::Answer},
    sheets::Page,
    test_events::{executed, said},
    theme::{self, Appearance},
};

const RENOA: Uuid = Uuid::from_u128(1);
const CHEF: Uuid = Uuid::from_u128(2);
const SCOUT: Uuid = Uuid::from_u128(3);

fn look(hair_front: HairFront, hair_colour: HairColour, wear: Wear, tile: Tile) -> Look {
    Look {
        hair_front,
        hair_back: HairBack::None,
        hair_colour,
        skin: Skin::Medium,
        eyes: Eyes::Oval,
        cheeks: Cheeks::Freckles,
        wear,
        tile,
        tuft: false,
    }
}

/// What the Host says to each call the screens make.
fn host(path: &str) -> Answer<String> {
    let now = chrono::Utc::now().timestamp_millis();
    let hour = 3_600_000;
    let body = match path {
        "/v1/host" => json!({
            "main_agent": RENOA,
            "automations": [
                {"id": "a", "agent_id": RENOA, "name": "Morning brief",
                 "schedule": {"kind": "cron", "expression": "30 7 * * 1-5", "timezone": "Asia/Kolkata"},
                 "enabled": true, "revision": 3, "next_due_ms": now + 9 * hour,
                 "last_run": {"status": "succeeded", "due_ms": now - 15 * hour, "finished_at_ms": now - 15 * hour + 40_000}},
                {"id": "b", "agent_id": CHEF, "name": "Plan the week's dinners",
                 "schedule": {"kind": "cron", "expression": "0 18 * * 0", "timezone": "Asia/Kolkata"},
                 "enabled": true, "revision": 1, "next_due_ms": now + 50 * hour,
                 "last_run": {"status": "failed", "due_ms": now - 120 * hour}},
                {"id": "c", "agent_id": SCOUT, "name": "Trending repos on GitHub",
                 "schedule": {"kind": "cron", "expression": "0 */6 * * *", "timezone": "Asia/Kolkata"},
                 "enabled": false, "revision": 2, "next_due_ms": now + 4 * hour},
                {"id": "d", "agent_id": RENOA, "name": "Call mum",
                 "schedule": {"kind": "once", "at": "2026-11-12T13:30:00Z"},
                 "enabled": true, "revision": 1, "next_due_ms": now + 70 * hour,
                 "plugin": "renoa.memory"}
            ]
        }),
        "/v1/host/feed" => {
            let item = |id: u128,
                        kind: &str,
                        agent: Uuid,
                        name: &str,
                        title: &str,
                        body: &str,
                        ago: i64,
                        choices: Option<Vec<&str>>| {
                json!({"id": Uuid::from_u128(id), "sequence": id, "kind": kind,
                    "agent": {"id": agent, "name": name}, "title": title, "body": body,
                    "at_ms": now - ago, "done": false, "action": "open",
                    "subject_id": Uuid::from_u128(500 + id), "urgency": "normal", "choices": choices})
            };
            json!({"today": "Chef has dinner ideas at 6:30 PM, and Scout found three repos worth a look.",
                "unread": 4, "seen_through": 0, "next_before": null, "items": [
                item(9, "ask", CHEF, "Chef", "Chef asks", "Paneer tikka or dal makhani tonight? Both use what's in the fridge.", 20 * 60_000, Some(vec!["Paneer tikka", "Dal makhani"])),
                item(8, "ask", RENOA, "Renoa", "Renoa asks", "Saturday's grocery list has 14 things. Want to look it over before I order?", 55 * 60_000, None),
                item(7, "update", SCOUT, "Scout", "Scout", "Three trending repos worth a look, one of them a GPUI component kit.", 3 * hour, None),
                item(6, "problem", CHEF, "Chef", "Plan the week's dinners didn't finish", "It can run again.", 5 * hour, None),
                item(5, "result", RENOA, "Renoa", "Morning brief ran", "See what it found.", 10 * hour, None),
            ]})
        }
        "/v1/host/profile" => json!({
            "revision": 4,
            "content": "**Yash**, lives in Bengaluru.\n\n- Vegetarian; no mushrooms.\n- Gym on weekday mornings.\n- Building Renoa in the evenings."
        }),
        "/v1/host/memory" => json!({
            "total": 14,
            "notes": [
                {"id": "1", "written": "2026-11-08", "author": "Chef", "text": "Liked the paneer tikka from Saturday; wants it again soon."},
                {"id": "2", "written": "2026-11-07", "author": "Renoa", "text": "Mum's birthday is on the 21st. Wants a reminder a week before.", "due": "2026-11-14T09:00"},
                {"id": "3", "written": "2026-11-06", "author": "Scout", "text": "Follows Rust GUI projects; skip JavaScript frameworks."},
                {"id": "4", "written": "2026-11-05", "text": "Groceries usually on Sunday evenings."}
            ]
        }),
        path if path.starts_with("/v1/host/agents/") && path.matches('/').count() == 4 => json!({
            "created_at_ms": now - 40 * 24 * hour
        }),
        path if path.ends_with("/apps") => {
            let on = path.contains(&RENOA.to_string());
            json!({"apps": [
                {"connection": "github", "name": "GitHub", "endpoint": "https://api.githubcopilot.com/mcp/", "on": on},
                {"connection": "calendar", "name": "Google Calendar", "endpoint": "https://calendar.example.com/mcp", "on": true},
                {"connection": "notion", "endpoint": "https://mcp.notion.com/mcp", "on": !on}
            ]})
        }
        _ => return Answer::Missing,
    };
    Answer::Said(body.to_string())
}

/// What each agent is for.
fn purpose(agent: Uuid) -> &'static str {
    match agent {
        CHEF => "Plans the week's dinners from what's in the fridge, and keeps the grocery list.",
        SCOUT => "Finds things worth a look: trending repos, deals and good reads.",
        _ => "Keeps your day in order and knows the rest of the team.",
    }
}

struct Screen {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    chats: Entity<Chats>,
    asked: mpsc::UnboundedReceiver<Command>,
    /// Held so the chats keep listening; a dropped sender reads as the
    /// session ending.
    _updates: mpsc::UnboundedSender<Update>,
}

impl Screen {
    fn open(appearance: Appearance) -> Self {
        let mut cx = HeadlessAppContext::with_platform(
            Arc::new(gpui_apple::AppleTextSystem::new()),
            Arc::new(crate::icons::Icons),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.update(gpui_kit::init);
        let (commands, asked) = mpsc::unbounded_channel();
        let (updates, received) = mpsc::unbounded_channel();
        let window = cx
            .open_window(size(px(1180.), px(780.)), |window, cx| {
                theme::apply(appearance, window, cx);
                cx.new(|cx| Chats::new(commands, received, "Yash's MacBook".into(), window, cx))
            })
            .expect("open a window");
        let node_id = NodeId::new();
        let agents = [(RENOA, "Renoa"), (CHEF, "Chef"), (SCOUT, "Scout")];
        updates.send(Update::Linked(true)).unwrap();
        updates
            .send(Update::Agents(
                agents
                    .iter()
                    .map(|(id, name)| TargetSummary {
                        node_id,
                        target: TargetRef::new(format!("agent:{id}")),
                        name: (*name).to_owned(),
                    })
                    .collect(),
            ))
            .unwrap();
        updates.send(Update::Main(Some(RENOA))).unwrap();
        for ((id, _), look) in agents.iter().zip([
            crate::face::look::RENOA,
            look(
                HairFront::Curls,
                HairColour::Auburn,
                Wear::Glasses,
                Tile::Ochre,
            ),
            look(
                HairFront::SidePart,
                HairColour::Brown,
                Wear::Headphones,
                Tile::Cobalt,
            ),
        ]) {
            updates
                .send(Update::Look {
                    node_id,
                    target: format!("agent:{id}"),
                    look,
                })
                .unwrap();
            updates
                .send(Update::Purpose {
                    node_id,
                    target: format!("agent:{id}"),
                    purpose: purpose(*id).to_owned(),
                })
                .unwrap();
        }
        let task_id = chat_task_id(node_id, &format!("agent:{RENOA}"));
        for event in [
            said(task_id, 0, CommandInput::from_text("What's on today?")),
            executed(task_id, 1, ExecutionEventKind::TurnStarted),
            executed(
                task_id,
                2,
                ExecutionEventKind::AssistantMessage {
                    text:
                        "Gym at 7, then the sheets for the Mac app. Chef planned paneer for dinner."
                            .to_owned(),
                },
            ),
            executed(
                task_id,
                3,
                ExecutionEventKind::ExecutionTerminated {
                    terminal: ExecutionTerminal::Completed,
                },
            ),
        ] {
            updates.send(Update::Event(Box::new(event))).unwrap();
        }
        let chats = cx.update(|cx| window.root(cx)).expect("the chats");
        let mut screen = Self {
            cx,
            window: window.into(),
            chats,
            asked,
            _updates: updates,
        };
        screen.settle();
        screen
    }

    /// Runs until the screens are waiting on nothing, answering each Host
    /// call as it comes.
    fn settle(&mut self) {
        loop {
            self.cx.run_until_parked();
            let mut answered = false;
            while let Ok(command) = self.asked.try_recv() {
                if let Command::Host(call) = command {
                    let _ = call.answered.send(host(&call.path));
                    answered = true;
                }
            }
            if !answered {
                break;
            }
        }
        self.cx.advance_clock(std::time::Duration::from_secs(1));
        self.cx.run_until_parked();
    }

    fn shoot(&mut self, name: &str) {
        let dir = PathBuf::from(std::env::var("RENOA_SCREENS").expect("RENOA_SCREENS"));
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.refresh();
                window.render_frame(cx);
            })
            .ok();
        let image = self
            .cx
            .capture_screenshot(self.window)
            .expect("a screenshot");
        image.save(dir.join(format!("{name}.png"))).expect("save");
    }
}

#[test]
#[ignore = "renders to files; run by hand"]
fn screens() {
    for (appearance, theme) in [(Appearance::Light, "light"), (Appearance::Dark, "dark")] {
        let mut screen = Screen::open(appearance);
        screen.shoot(&format!("home-{theme}"));
        let chats = screen.chats.clone();
        screen
            .cx
            .update_window(screen.window, |_, window, cx| {
                chats.update(cx, |chats, cx| chats.open_agent(0, window, cx));
            })
            .unwrap();
        screen.settle();
        screen.shoot(&format!("chats-{theme}"));
        for page in Page::ALL {
            let chats = screen.chats.clone();
            screen
                .cx
                .update_window(screen.window, |_, window, cx| {
                    chats.update(cx, |chats, cx| chats.open_sheet(page, window, cx));
                })
                .unwrap();
            screen.settle();
            screen.shoot(&format!("{}-{theme}", page.title().to_lowercase()));
            let chats = screen.chats.clone();
            screen
                .cx
                .update_window(screen.window, |_, window, cx| {
                    chats.update(cx, |chats, cx| chats.close(window, cx));
                })
                .unwrap();
        }
        let chats = screen.chats.clone();
        screen
            .cx
            .update_window(screen.window, |_, window, cx| {
                chats.update(cx, |chats, cx| {
                    let chef = chats.members()[1].task_id;
                    chats.open_profile(chef, window, cx);
                });
            })
            .unwrap();
        screen.settle();
        screen.shoot(&format!("profile-{theme}"));
        let chats = screen.chats.clone();
        screen
            .cx
            .update_window(screen.window, |_, window, cx| {
                chats.update(cx, |chats, cx| chats.open_jump(window, cx));
            })
            .unwrap();
        screen.settle();
        screen.shoot(&format!("jump-{theme}"));
    }
}

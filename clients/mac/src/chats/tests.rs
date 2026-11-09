use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds,
    WindowOptions, px, size, test::TestWindowExt as _,
};
use renoa_protocol::{CommandInput, ExecutionEventKind, ExecutionTerminal, TargetRef};
use renoa_rcp_wire::{NodeId, TargetSummary};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{Chats, overlay::Up};
use crate::{
    chat::{Speaker, chat_task_id},
    face::look::PLAIN,
    link::{Command, Update},
    test_events::{executed, said},
};

type Opened = (
    AnyWindowHandle,
    Entity<Chats>,
    mpsc::UnboundedReceiver<Command>,
    mpsc::UnboundedSender<Update>,
);

fn open(cx: &mut TestAppContext) -> Opened {
    cx.update(gpui_kit::init);
    cx.update(super::keys::bind);
    let (commands, asked) = mpsc::unbounded_channel();
    let (updates, received) = mpsc::unbounded_channel();
    let (window, chats) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: size(px(900.), px(600.)),
                })),
                ..WindowOptions::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let mut chats = Chats::new(commands, received, "Test Mac".into(), window, cx);
                    // These tests are about the chats, so one shows rather
                    // than Home.
                    chats.on_home = false;
                    chats
                })
            },
        )
        .expect("open a window")
    });
    (window, chats, asked, updates)
}

/// The next command about the chats, past Home's reads of the Host.
fn chat_command(
    asked: &mut mpsc::UnboundedReceiver<Command>,
) -> Result<Command, mpsc::error::TryRecvError> {
    loop {
        match asked.try_recv()? {
            Command::Host(_) => {}
            command => return Ok(command),
        }
    }
}

fn agent(node_id: NodeId, name: &str) -> TargetSummary {
    TargetSummary {
        node_id,
        target: TargetRef::new(name.to_lowercase()),
        name: name.to_owned(),
    }
}

#[gpui_kit::test]
fn the_chats_follow_the_first_agent_show_its_turn_and_send_what_the_owner_types(
    cx: &mut TestAppContext,
) {
    let (window, chats, mut asked, updates) = open(cx);
    let node_id = NodeId::new();
    let task_id = chat_task_id(node_id, "renoa");
    updates.send(Update::Linked(true)).unwrap();
    updates
        .send(Update::Agents(vec![TargetSummary {
            node_id,
            target: TargetRef::new("renoa"),
            name: "Renoa".to_owned(),
        }]))
        .unwrap();
    cx.run_until_parked();
    let Ok(Command::Follow {
        task_id: followed,
        after: None,
        ..
    }) = chat_command(&mut asked)
    else {
        panic!("the first agent's chat is followed from its start");
    };
    assert_eq!(followed, task_id);

    for event in [
        said(task_id, 0, CommandInput::from_text("What's for dinner?")),
        executed(task_id, 1, ExecutionEventKind::TurnStarted),
    ] {
        updates.send(Update::Event(Box::new(event))).unwrap();
    }
    cx.run_until_parked();
    chats.read_with(cx, |chats, _| {
        assert_eq!(chats.rows(), 2, "the owner's line, then the agent at work");
    });
    updates
        .send(Update::Event(Box::new(executed(
            task_id,
            2,
            ExecutionEventKind::AssistantMessage {
                text: "Pasta.".to_owned(),
            },
        ))))
        .unwrap();
    cx.run_until_parked();
    chats.read_with(cx, |chats, cx| {
        let lines = chats.transcripts[&task_id].lines();
        assert_eq!(
            lines
                .iter()
                .map(|line| (line.speaker, line.text.as_str()))
                .collect::<Vec<_>>(),
            [
                (Speaker::Owner, "What's for dinner?"),
                (Speaker::Agent, "Pasta.")
            ]
        );
        assert_eq!(chats.scroller.read(cx).item_count(), 3);
    });
    updates
        .send(Update::Event(Box::new(executed(
            task_id,
            3,
            ExecutionEventKind::ExecutionTerminated {
                terminal: ExecutionTerminal::Completed,
            },
        ))))
        .unwrap();
    cx.run_until_parked();
    chats.read_with(cx, |chats, cx| {
        assert_eq!(
            chats.scroller.read(cx).item_count(),
            2,
            "the working row goes once the agent is done"
        );
    });

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer", cx);
        window.input("Make it vegetarian", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let Ok(Command::Send {
        task_id: sent_to,
        text,
        sent: None,
    }) = chat_command(&mut asked)
    else {
        panic!("Enter sends the message");
    };
    assert_eq!((sent_to, text.as_str()), (task_id, "Make it vegetarian"));
    chats.read_with(cx, |chats, cx| {
        assert!(chats.composer.read(cx).value().is_empty())
    });
}

#[gpui_kit::test]
fn each_chat_keeps_its_own_draft_and_a_refused_message_returns_to_its_chat(
    cx: &mut TestAppContext,
) {
    let (window, chats, mut asked, updates) = open(cx);
    let node_id = NodeId::new();
    let (cook, shopper) = (agent(node_id, "Cook"), agent(node_id, "Shopper"));
    let cook_chat = chat_task_id(node_id, "cook");
    let shopper_chat = chat_task_id(node_id, "shopper");
    updates
        .send(Update::Agents(vec![cook.clone(), shopper.clone()]))
        .unwrap();
    cx.run_until_parked();
    while chat_command(&mut asked).is_ok() {}
    let composer = |cx: &mut TestAppContext| {
        chats.read_with(cx, |chats, cx| chats.composer.read(cx).value().to_string())
    };
    let select = |index: usize, cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            chats.update(cx, |chats, cx| chats.select(index, window, cx));
        })
        .unwrap();
        cx.run_until_parked();
    };

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer", cx);
        window.input("half a thought", cx);
    })
    .unwrap();
    select(1, cx);
    assert_eq!(composer(cx), "", "the shopper's chat has no draft");
    updates
        .send(Update::NotSent {
            task_id: cook_chat,
            text: "lost".to_owned(),
        })
        .unwrap();
    updates
        .send(Update::NotSent {
            task_id: shopper_chat,
            text: "Buy milk".to_owned(),
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        composer(cx),
        "Buy milk",
        "the refused message comes back to its chat"
    );
    chats.read_with(cx, |chats, _| assert!(chats.unsent.contains(&shopper_chat)));

    select(0, cx);
    assert_eq!(
        composer(cx),
        "lost half a thought",
        "a refused message comes back ahead of the owner's draft"
    );

    while chat_command(&mut asked).is_ok() {}
    // The cook goes offline and the list comes back without it.
    updates.send(Update::Agents(vec![shopper])).unwrap();
    cx.run_until_parked();
    chats.read_with(cx, |chats, _| {
        assert_eq!(
            chats.chosen_task(),
            Some(cook_chat),
            "the open chat stays open"
        );
    });
    assert!(
        chat_command(&mut asked).is_err(),
        "no other chat was opened"
    );
}

#[gpui_kit::test]
fn what_the_owner_types_waits_while_renoa_is_away(cx: &mut TestAppContext) {
    let (window, chats, mut asked, updates) = open(cx);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click("composer", cx);
        window.input("Early thought", cx);
    })
    .unwrap();
    updates
        .send(Update::Agents(vec![agent(NodeId::new(), "Cook")]))
        .unwrap();
    cx.run_until_parked();
    let composer = |cx: &mut TestAppContext| {
        chats.read_with(cx, |chats, cx| chats.composer.read(cx).value().to_string())
    };
    assert_eq!(composer(cx), "Early thought", "the first chat keeps it");
    while chat_command(&mut asked).is_ok() {}

    updates.send(Update::Linked(false)).unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.click("composer", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(
        chat_command(&mut asked).is_err(),
        "nothing is sent while offline"
    );
    assert_eq!(
        composer(cx),
        "Early thought",
        "the text waits in the composer"
    );
}

#[gpui_kit::test]
fn a_chat_shows_what_its_agent_is_for_until_something_is_said(cx: &mut TestAppContext) {
    let (_window, chats, _asked, updates) = open(cx);
    let node_id = NodeId::new();
    let task_id = chat_task_id(node_id, "ada");
    updates
        .send(Update::Agents(vec![agent(node_id, "Ada")]))
        .unwrap();
    updates
        .send(Update::Purpose {
            node_id,
            target: "ada".to_owned(),
            purpose: "Plans dinners".to_owned(),
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        chats.read_with(cx, |chats, _| chats.preview(task_id)),
        "Plans dinners"
    );

    updates
        .send(Update::Event(Box::new(said(
            task_id,
            0,
            CommandInput::from_text("Tacos tonight?"),
        ))))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        chats.read_with(cx, |chats, _| chats.preview(task_id)),
        "Tacos tonight?"
    );
}

#[gpui_kit::test]
fn each_agent_wears_its_look_and_the_main_agent_is_marked(cx: &mut TestAppContext) {
    const ADA: &str = "1b4e28ba-2fa1-11d2-883f-0016d3cca427";
    const BO: &str = "6fa459ea-ee8a-3ca4-894e-db77e160355e";
    let (_window, chats, _asked, updates) = open(cx);
    let node_id = NodeId::new();
    let listed = |id: &str, name: &str| TargetSummary {
        node_id,
        target: TargetRef::new(format!("agent:{id}")),
        name: name.to_owned(),
    };
    let (ada, bo) = (listed(ADA, "Ada"), listed(BO, "Bo"));
    updates
        .send(Update::Agents(vec![ada.clone(), bo.clone()]))
        .unwrap();
    updates
        .send(Update::Look {
            node_id,
            target: format!("agent:{ADA}"),
            look: PLAIN,
        })
        .unwrap();
    cx.run_until_parked();
    let faces = |cx: &mut TestAppContext| {
        chats.read_with(cx, |chats, _| (chats.face(&ada), chats.face(&bo)))
    };
    assert_eq!(
        faces(cx),
        ((Some(PLAIN), true), (None, false)),
        "the first listed stands in before the Host names one"
    );

    updates
        .send(Update::Main(Some(Uuid::parse_str(BO).unwrap())))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(faces(cx), ((Some(PLAIN), false), (None, true)));

    updates.send(Update::Main(Some(Uuid::new_v4()))).unwrap();
    cx.run_until_parked();
    assert_eq!(
        faces(cx),
        ((Some(PLAIN), true), (None, false)),
        "one named but not listed leaves the first to stand in"
    );
}

#[gpui_kit::test]
fn home_shows_first_with_the_keys_and_a_chat_opened_from_the_side_is_ready_to_type(
    cx: &mut TestAppContext,
) {
    use gpui_kit::Focusable as _;

    use crate::sheets::Page;

    let (window, chats, _asked, updates) = open(cx);
    chats.update(cx, |chats, _| chats.on_home = true);
    let node_id = NodeId::new();
    updates
        .send(Update::Agents(vec![
            agent(node_id, "Cook"),
            agent(node_id, "Shopper"),
        ]))
        .unwrap();
    cx.run_until_parked();
    let focused = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            let chats = chats.read(cx);
            let typing = chats.composer.read(cx).focus_handle(cx).is_focused(window);
            (chats.on_home, chats.home_focus.is_focused(window), typing)
        })
        .unwrap()
    };
    let up = |cx: &mut TestAppContext| {
        chats.read_with(cx, |chats, _| chats.sheet.as_ref().map(|shown| shown.up))
    };
    let press = |keys: &'static str, cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.press(keys, cx);
        })
        .unwrap();
        cx.run_until_parked();
    };
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert_eq!(
        focused(cx),
        (true, true, false),
        "Home has the keys from launch"
    );
    press("cmd-shift-r", cx);
    assert_eq!(
        up(cx),
        Some(Up::Sheet(Page::Routines)),
        "the keys work on Home"
    );
    press("escape", cx);
    assert_eq!(up(cx), None);
    assert_eq!(
        focused(cx),
        (true, true, false),
        "closing gives Home the keys back"
    );

    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.click(("agent", 1usize), cx);
        window.input("Oat milk", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        focused(cx),
        (false, false, true),
        "the chat is ready to type"
    );
    chats.read_with(cx, |chats, cx| {
        assert_eq!(
            chats.chosen.as_ref().map(|chosen| chosen.name.as_ref()),
            Some("Shopper")
        );
        assert_eq!(chats.composer.read(cx).value().as_ref(), "Oat milk");
    });
    press("cmd-0", cx);
    assert_eq!(focused(cx), (true, true, false), "⌘0 goes Home");
    press("cmd-1", cx);
    assert_eq!(focused(cx), (false, false, true));
    chats.read_with(cx, |chats, _| {
        assert_eq!(
            chats.chosen.as_ref().map(|chosen| chosen.name.as_ref()),
            Some("Cook")
        );
    });
}

#[gpui_kit::test]
fn an_agents_page_opens_from_its_chat_and_leads_to_its_chat_and_the_apps(cx: &mut TestAppContext) {
    use crate::{
        link::host::Answer,
        sheets::{Page, profile::Profile},
    };

    let (window, chats, mut asked, updates) = open(cx);
    let node_id = NodeId::new();
    let chef = format!("agent:{}", Uuid::from_u128(0xc1));
    let task_id = chat_task_id(node_id, &chef);
    updates
        .send(Update::Agents(vec![
            agent(node_id, "Cook"),
            TargetSummary {
                node_id,
                target: TargetRef::new(chef.clone()),
                name: "Chef".to_owned(),
            },
        ]))
        .unwrap();
    updates
        .send(Update::Purpose {
            node_id,
            target: chef.clone(),
            purpose: "Plans dinners.".to_owned(),
        })
        .unwrap();
    cx.run_until_parked();
    let up = |cx: &mut TestAppContext| {
        chats.read_with(cx, |chats, _| chats.sheet.as_ref().map(|shown| shown.up))
    };
    let mut act = |act: &dyn Fn(&mut gpui_kit::Window, &mut gpui_kit::App),
                   cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            act(window, cx);
        })
        .unwrap();
        loop {
            cx.run_until_parked();
            let mut served = false;
            while let Ok(command) = asked.try_recv() {
                if let Command::Host(call) = command {
                    served = true;
                    let said = match call.path.as_str() {
                        "/v1/host" => r#"{"automations":[]}"#,
                        "/v1/host/feed" => r#"{"today":"","items":[]}"#,
                        path if path.ends_with("/apps") => r#"{"apps":[]}"#,
                        _ => r#"{"created_at_ms":0,"operational":{}}"#,
                    };
                    let _ = call.answered.send(Answer::Said(said.to_owned()));
                }
            }
            if !served {
                break;
            }
        }
    };
    act(&|window, cx| window.click(("agent", 0usize), cx), cx);
    act(&|window, cx| window.press("cmd-i", cx), cx);
    assert_eq!(up(cx), None, "the first agent is not one of the Host's");
    act(&|window, cx| window.click("about", cx), cx);
    assert_eq!(up(cx), None, "nor is its name a way to a page");
    act(&|window, cx| window.click(("agent", 1usize), cx), cx);
    act(&|window, cx| window.press("cmd-i", cx), cx);
    assert_eq!(up(cx), Some(Up::Profile(task_id)), "⌘I opens it");
    let purpose = chats.read_with(cx, |chats, cx| {
        let shown = chats.sheet.as_ref().expect("a sheet");
        let profile = shown.view.clone().downcast::<Profile>().expect("the page");
        profile.read(cx).purpose().cloned()
    });
    assert_eq!(
        purpose.as_deref(),
        Some("Plans dinners."),
        "what the chats know it is for"
    );
    act(&|window, cx| window.click("manage-apps", cx), cx);
    assert_eq!(up(cx), Some(Up::Sheet(Page::Apps)), "Manage opens Apps");
    act(&|window, cx| window.press("escape", cx), cx);
    act(&|window, cx| window.click("about", cx), cx);
    assert_eq!(
        up(cx),
        Some(Up::Profile(task_id)),
        "so does its name at the top"
    );
    act(&|window, cx| window.click("all-routines", cx), cx);
    assert_eq!(
        up(cx),
        Some(Up::Sheet(Page::Routines)),
        "All routines opens Routines"
    );
    act(&|window, cx| window.press("cmd-0", cx), cx);
    act(&|window, cx| window.press("cmd-i", cx), cx);
    assert_eq!(up(cx), None, "Home has no agent to show");
    act(&|window, cx| window.click(("agent", 1usize), cx), cx);
    act(&|window, cx| window.press("cmd-i", cx), cx);
    act(&|window, cx| window.click("message", cx), cx);
    chats.read_with(cx, |chats, _| {
        assert!(chats.sheet.is_none());
        assert_eq!(
            chats.chosen.as_ref().map(|chosen| chosen.task_id),
            Some(task_id)
        );
    });
}

#[gpui_kit::test]
fn each_key_settings_lists_does_what_it_says(cx: &mut TestAppContext) {
    use crate::{
        chats::keys::AboutAgent,
        sheets::{Page, settings::SHORTCUTS},
    };

    let (window, chats, _asked, updates) = open(cx);
    let node_id = NodeId::new();
    let chef = format!("agent:{}", Uuid::from_u128(0xc1));
    let chef_chat = chat_task_id(node_id, &chef);
    updates
        .send(Update::Agents(vec![
            TargetSummary {
                node_id,
                target: TargetRef::new(chef.clone()),
                name: "Chef".to_owned(),
            },
            agent(node_id, "Cook"),
        ]))
        .unwrap();
    cx.run_until_parked();
    let act = |act: &dyn Fn(&mut gpui_kit::Window, &mut gpui_kit::App), cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            act(window, cx);
        })
        .unwrap();
        cx.run_until_parked();
    };
    // Where the owner is: Home, the sheet or page that is up, the jump box,
    // and the chat that is open.
    let at = |cx: &mut TestAppContext| {
        chats.read_with(cx, |chats, _| {
            (
                chats.on_home,
                chats.sheet.as_ref().map(|shown| shown.up),
                chats.jumping.is_some(),
                chats.chosen.as_ref().map(|chosen| chosen.name.to_string()),
            )
        })
    };
    let pressed = |keys: &str| match keys {
        "⌘K" => "cmd-k",
        "⌘0" => "cmd-0",
        "⌘I" => "cmd-i",
        "⌘1 – ⌘9" => "cmd-1",
        "⌘⇧R" => "cmd-shift-r",
        "⌘⇧M" => "cmd-shift-m",
        "⌘⇧A" => "cmd-shift-a",
        "⌘," => "cmd-,",
        "Esc" => "escape",
        other => panic!("Settings lists {other}, which this test does not press"),
    };
    let chat = |name: &str| Some(name.to_owned());
    for (what, keys) in SHORTCUTS {
        // From Chef's chat, ready to type; Esc from over a sheet.
        act(&|window, cx| window.click(("agent", 0usize), cx), cx);
        if what == "Close a sheet" {
            act(&|window, cx| window.press("cmd-shift-m", cx), cx);
        }
        let expected = match what {
            "Jump to an agent or page" => (false, None, true, chat("Chef")),
            "Home" => (true, None, false, chat("Chef")),
            "About the agent you're talking to" => {
                (false, Some(Up::Profile(chef_chat)), false, chat("Chef"))
            }
            "Open the first nine agents" => {
                act(&|window, cx| window.click(("agent", 1usize), cx), cx);
                (false, None, false, chat("Chef"))
            }
            "Close a sheet" => (false, None, false, chat("Chef")),
            page => {
                let page = Page::ALL
                    .into_iter()
                    .find(|listed| listed.title() == page)
                    .unwrap_or_else(|| panic!("no page named {page}"));
                (false, Some(Up::Sheet(page)), false, chat("Chef"))
            }
        };
        act(&|window, cx| window.press(pressed(keys), cx), cx);
        assert_eq!(at(cx), expected, "{what}: {keys}");
        act(&|window, cx| window.press("escape", cx), cx);
    }
    for page in Page::ALL {
        assert!(
            SHORTCUTS
                .iter()
                .any(|(what, keys)| *what == page.title() && *keys == page.keys()),
            "{} shows its keys in Settings",
            page.title()
        );
    }

    // The keys reach the chats from Home and from the jump box's own field.
    act(&|window, cx| window.press("cmd-0", cx), cx);
    act(&|window, cx| window.press("cmd-shift-r", cx), cx);
    assert_eq!(
        at(cx),
        (true, Some(Up::Sheet(Page::Routines)), false, chat("Chef"))
    );
    act(&|window, cx| window.press("escape", cx), cx);
    act(&|window, cx| window.press("cmd-k", cx), cx);
    act(&|window, cx| window.press("cmd-0", cx), cx);
    assert_eq!(
        at(cx),
        (true, None, false, chat("Chef")),
        "from the jump box"
    );

    // About This Agent is there only while a page can open.
    let about = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.is_action_available(&AboutAgent, cx)
        })
        .unwrap()
    };
    assert!(!about(cx), "not on Home");
    act(&|window, cx| window.click(("agent", 1usize), cx), cx);
    assert!(!about(cx), "not for an agent that is not the Host's");
    act(&|window, cx| window.click(("agent", 0usize), cx), cx);
    assert!(about(cx));
}

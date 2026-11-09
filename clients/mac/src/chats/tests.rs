use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds,
    WindowOptions, px, size, test::TestWindowExt as _,
};
use renoa_protocol::{CommandInput, ExecutionEventKind, ExecutionTerminal, TargetRef};
use renoa_rcp_wire::{NodeId, TargetSummary};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::Chats;
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
            |window, cx| cx.new(|cx| Chats::new(commands, received, window, cx)),
        )
        .expect("open a window")
    });
    (window, chats, asked, updates)
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
    }) = asked.try_recv()
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
    }) = asked.try_recv()
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
    while asked.try_recv().is_ok() {}
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

    while asked.try_recv().is_ok() {}
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
    assert!(asked.try_recv().is_err(), "no other chat was opened");
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
    while asked.try_recv().is_ok() {}

    updates.send(Update::Linked(false)).unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
        window.click("composer", cx);
        window.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert!(asked.try_recv().is_err(), "nothing is sent while offline");
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
        ((Some(PLAIN), false), (None, false)),
        "no agent is main before the Host names one"
    );

    updates
        .send(Update::Main(Some(Uuid::parse_str(BO).unwrap())))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(faces(cx), ((Some(PLAIN), false), (None, true)));
}

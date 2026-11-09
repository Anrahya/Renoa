//! Home against a Host the test answers, call by call.

use gpui_kit::{AppContext as _, Entity, TestAppContext};
use renoa_rcp_wire::{HostMethod, TaskId};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::{Home, soonest};
use crate::{
    chat::chat_task_id,
    link::{Command, host::Answer},
    sheets::{Member, routines::held::Observation},
};

#[test]
fn coming_up_is_the_soonest_routines_that_are_on() {
    let routine = |id: &str, enabled: bool, next: i64| {
        serde_json::json!({"id": id, "agent_id": "1b4e28ba-2fa1-11d2-883f-0016d3cca427",
            "name": id, "schedule": {"kind": "cron", "expression": "0 9 * * *",
            "timezone": "Asia/Kolkata"}, "enabled": enabled, "revision": 1,
            "next_due_ms": next})
    };
    let observed: Observation = serde_json::from_value(serde_json::json!({"automations": [
        routine("late", true, 40), routine("paused", false, 1), routine("soon", true, 10),
        routine("next", true, 20), routine("later", true, 30)
    ]}))
    .expect("an observation");
    let names: Vec<String> = soonest(observed.automations)
        .into_iter()
        .map(|routine| routine.name)
        .collect();
    assert_eq!(names, ["soon", "next", "later"]);
}

const DEALS: Uuid = Uuid::from_u128(0xa1);
const ASKED_HERE: Uuid = Uuid::from_u128(1);
const ASKED_ELSEWHERE: Uuid = Uuid::from_u128(2);
const PROBLEM: Uuid = Uuid::from_u128(3);

fn deals_chat() -> TaskId {
    chat_task_id(
        renoa_rcp_wire::NodeId::from_uuid(Uuid::from_u128(9)),
        "agent:deals",
    )
}

fn page() -> String {
    let item = |id: Uuid, kind: &str, subject: String| {
        serde_json::json!({"id": id, "sequence": 1, "kind": kind,
            "agent": {"id": DEALS, "name": "Deals"}, "title": "Deals", "body": "Add two?",
            "at_ms": 1, "done": false, "action": "answer", "subject_id": subject,
            "urgency": "normal", "choices": ["Add two", "Not now"]})
    };
    serde_json::json!({"today": "A quiet day.", "unread": 0, "seen_through": 0, "items": [
        item(ASKED_HERE, "ask", deals_chat().as_uuid().to_string()),
        item(PROBLEM, "problem", "routine-1".to_owned()),
    ], "next_before": null})
    .to_string()
}

/// The page after the question asked here was answered: one asked in
/// another conversation is the agent's open question now.
fn later_page() -> String {
    page()
        .replace(&ASKED_HERE.to_string(), &ASKED_ELSEWHERE.to_string())
        .replace(
            &deals_chat().as_uuid().to_string(),
            &Uuid::from_u128(77).to_string(),
        )
}

struct Scripted {
    home: Entity<Home>,
    asked: mpsc::UnboundedReceiver<Command>,
}

impl Scripted {
    fn open(cx: &mut TestAppContext) -> Self {
        let (commands, asked) = mpsc::unbounded_channel();
        let member = Member {
            name: "Deals".into(),
            host_id: DEALS,
            task_id: deals_chat(),
            look: None,
            primary: false,
        };
        let home = cx.new(|cx| Home::new(commands, vec![member], cx));
        Self { home, asked }
    }

    /// Answers the Host calls asked so far with `answer(method, path)`, and
    /// returns everything else asked.
    fn serve(
        &mut self,
        cx: &mut TestAppContext,
        answer: impl Fn(HostMethod, &str, Option<&str>) -> Answer<String>,
    ) -> Vec<Command> {
        let mut others = Vec::new();
        loop {
            cx.run_until_parked();
            let mut served = false;
            while let Ok(command) = self.asked.try_recv() {
                match command {
                    Command::Host(call) => {
                        served = true;
                        let _ = call.answered.send(answer(
                            call.method,
                            &call.path,
                            call.body.as_deref(),
                        ));
                    }
                    other => others.push(other),
                }
            }
            if !served {
                return others;
            }
        }
    }
}

fn host(page: String) -> impl Fn(HostMethod, &str, Option<&str>) -> Answer<String> {
    move |_, path, _| match path {
        "/v1/host/feed" => Answer::Said(page.clone()),
        "/v1/host" => Answer::Said(r#"{"automations":[]}"#.to_owned()),
        _ => Answer::Said("{}".to_owned()),
    }
}

fn asks(home: &Entity<Home>, cx: &mut TestAppContext) -> Vec<Uuid> {
    home.read_with(cx, |home, _| match &home.feed {
        crate::sheets::Loaded::Ready(held) => held.asks().iter().map(|ask| ask.id).collect(),
        _ => Vec::new(),
    })
}

#[gpui_kit::test]
fn an_answer_goes_to_the_agents_chat_and_one_from_elsewhere_names_and_puts_away_its_question(
    cx: &mut TestAppContext,
) {
    let mut scripted = Scripted::open(cx);
    scripted.serve(cx, host(page()));
    assert_eq!(asks(&scripted.home, cx), [ASKED_HERE]);

    scripted.home.update(cx, |home, cx| {
        home.answer(ASKED_HERE, "Add two".to_owned(), cx)
    });
    let sent = scripted.serve(cx, host(page()));
    let [
        Command::Send {
            task_id,
            text,
            sent: Some(took),
        },
    ] = <[Command; 1]>::try_from(sent).unwrap_or_else(|sent| panic!("one message: {sent:?}"))
    else {
        panic!("the answer is a message");
    };
    assert_eq!((task_id, text.as_str()), (deals_chat(), "Add two"));
    assert!(
        asks(&scripted.home, cx).is_empty(),
        "the question hides at once"
    );
    took.send(true).expect("Home listens");
    let dismissed = std::sync::Mutex::new(Vec::new());
    scripted.serve(cx, |method, path, _| {
        dismissed
            .lock()
            .expect("dismissed")
            .push((method, path.to_owned()));
        Answer::Said("{}".to_owned())
    });
    assert!(
        dismissed.into_inner().expect("dismissed").is_empty(),
        "the Host marks a question answered in its chat"
    );

    scripted.home.update(cx, |home, cx| home.read(cx));
    scripted.serve(cx, host(later_page()));
    scripted.home.update(cx, |home, cx| {
        home.answer(ASKED_ELSEWHERE, "Not now".to_owned(), cx)
    });
    let sent = scripted.serve(cx, host(later_page()));
    let [
        Command::Send {
            text,
            sent: Some(took),
            ..
        },
    ] = <[Command; 1]>::try_from(sent).unwrap_or_else(|sent| panic!("one message: {sent:?}"))
    else {
        panic!("the answer is a message");
    };
    assert_eq!(text, "About your question “Add two?”: Not now");
    took.send(true).expect("Home listens");
    let dismissed = std::sync::Mutex::new(Vec::new());
    scripted.serve(cx, |method, path, _| {
        dismissed
            .lock()
            .expect("dismissed")
            .push((method, path.to_owned()));
        Answer::Said("{}".to_owned())
    });
    assert_eq!(
        dismissed.into_inner().expect("dismissed"),
        [(
            HostMethod::Post,
            format!("/v1/host/feed/items/{ASKED_ELSEWHERE}/dismissal")
        )]
    );
}

#[gpui_kit::test]
fn a_refused_answer_shows_its_question_again(cx: &mut TestAppContext) {
    let mut scripted = Scripted::open(cx);
    scripted.serve(cx, host(page()));
    scripted.home.update(cx, |home, cx| {
        home.answer(ASKED_HERE, "Add two".to_owned(), cx)
    });
    let sent = scripted.serve(cx, host(page()));
    let Some(Command::Send {
        sent: Some(took), ..
    }) = sent.into_iter().next()
    else {
        panic!("the answer is a message");
    };
    took.send(false).expect("Home listens");
    scripted.serve(cx, host(page()));
    assert_eq!(asks(&scripted.home, cx), [ASKED_HERE]);
    assert!(
        scripted
            .home
            .read_with(cx, |home, _| home.refused.is_some())
    );
}

#[gpui_kit::test]
fn a_failed_routine_run_again_keeps_its_run_until_the_host_confirms_it(cx: &mut TestAppContext) {
    let mut scripted = Scripted::open(cx);
    scripted.serve(cx, host(page()));
    let runs = std::sync::Mutex::new(Vec::new());
    let run = |scripted: &mut Scripted, answer: Answer<String>, cx: &mut TestAppContext| {
        scripted
            .home
            .update(cx, |home, cx| home.run_again(PROBLEM, cx));
        scripted.serve(cx, |method, path, body| {
            if method == HostMethod::Post {
                runs.lock()
                    .expect("runs")
                    .push((path.to_owned(), body.unwrap_or_default().to_owned()));
            }
            answer.clone()
        });
    };
    run(&mut scripted, Answer::Lost, cx);
    let first = scripted
        .home
        .read_with(cx, |home, _| home.runs.get(&PROBLEM).copied());
    run(&mut scripted, Answer::Said("{}".to_owned()), cx);
    let first = first.expect("an unconfirmed run keeps its id");
    let asked = format!(r#"{{"operation_id":"{first}"}}"#);
    assert_eq!(
        runs.into_inner().expect("runs"),
        vec![("/v1/host/automations/routine-1/run".to_owned(), asked); 2],
        "asked again, it is the same run"
    );
    scripted.home.read_with(cx, |home, _| {
        assert!(home.runs.is_empty(), "a confirmed run is done with");
        let crate::sheets::Loaded::Ready(held) = &home.feed else {
            panic!("the feed is read");
        };
        assert!(held.item(PROBLEM).is_none(), "its note is put away");
    });
}

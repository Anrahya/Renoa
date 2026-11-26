//! The session against a fake coordinator that answers as a test scripts it.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use futures_util::{SinkExt as _, StreamExt as _};
use renoa_protocol::{CommandId, CommandInput, ExecutionEventKind, ExecutionTerminal, TargetRef};
use renoa_rcp_client::StopOutcome;
use renoa_rcp_wire::{
    ClientMessage, DeviceCredential, DeviceCredentials, DeviceId, ErrorCode, HeldNotice,
    HostMethod, JSON_WS_VERSION, NodeId, ServerMessage, ShareLink, TargetSummary, TaskId,
};
use tokio::{
    net::TcpListener,
    sync::{mpsc, oneshot},
    time::timeout,
};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use uuid::Uuid;

use super::{
    Command, Endpoints, Pace, StopProblem, Update,
    host::{Answer, HostCall},
    session,
};
use crate::{
    face::look::HairFront,
    key::MacKey,
    notice::{NoticeSecret, SetupLinkKind, now_ms, tests::seal_for_test},
    test_events::{executed, said},
};

const QUICK: Pace = Pace {
    collect_every: Duration::from_millis(5),
    retry_after: Duration::from_millis(5),
    agents_every: Duration::from_millis(20),
};

/// What the fake coordinator sends back for one frame: text frames.
type Reply = String;

/// A frame no client can read, which ends the connection.
const GARBAGE: &str = "not a frame";

fn reply(message: &ServerMessage) -> Reply {
    serde_json::to_string(message).expect("encode")
}

/// Which connection a frame came on, and the frame.
type Heard = Arc<Mutex<Vec<(usize, ClientMessage)>>>;

type Script = dyn Fn(usize, &ClientMessage) -> Vec<Reply> + Send + Sync;

/// A coordinator that authenticates every connection and answers every other
/// frame as `script` says, given the connection's number.
async fn coordinator(script: Arc<Script>) -> (Endpoints, Heard) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let endpoints = Endpoints::at(&format!(
        "http://{}",
        listener.local_addr().expect("address")
    ));
    let heard = Heard::default();
    let log = Arc::clone(&heard);
    tokio::spawn(async move {
        let mut connections = 0;
        while let Ok((stream, _)) = listener.accept().await {
            let number = connections;
            connections += 1;
            let (script, log) = (Arc::clone(&script), Arc::clone(&log));
            tokio::spawn(async move {
                let Ok(mut socket) = accept_async(stream).await else {
                    return;
                };
                while let Some(Ok(Message::Text(text))) = socket.next().await {
                    let frame: ClientMessage = serde_json::from_str(&text).expect("client frame");
                    let replies = match &frame {
                        ClientMessage::Authenticate { .. } => {
                            vec![reply(&ServerMessage::Authenticated {
                                version: JSON_WS_VERSION,
                            })]
                        }
                        other => script(number, other),
                    };
                    log.lock().expect("log").push((number, frame));
                    for text in replies {
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            return;
                        }
                    }
                }
            });
        }
    });
    (endpoints, heard)
}

/// The answers every chat needs: the agent list, notice setup, the task and the attach.
fn chat_replies(frame: &ClientMessage) -> Option<Vec<Reply>> {
    let answer = match frame {
        ClientMessage::ListTargets { request_id } => ServerMessage::TargetList {
            request_id: *request_id,
            targets: Vec::new(),
        },
        ClientMessage::SetNoticeKey { request_id, .. } => ServerMessage::NoticeKeySet {
            request_id: *request_id,
        },
        ClientMessage::ListNotices { request_id } => ServerMessage::NoticeList {
            request_id: *request_id,
            notices: Vec::new(),
        },
        ClientMessage::AcknowledgeNotice { request_id, .. } => ServerMessage::NoticeAcknowledged {
            request_id: *request_id,
        },
        ClientMessage::OpenTask {
            request_id,
            task_id,
            ..
        } => ServerMessage::TaskOpened {
            request_id: *request_id,
            task_id: *task_id,
        },
        ClientMessage::Attach {
            request_id,
            task_id,
            ..
        } => ServerMessage::Attached {
            request_id: *request_id,
            task_id: *task_id,
            through_sequence: None,
        },
        _ => return None,
    };
    Some(vec![reply(&answer)])
}

fn accepted(frame: &ClientMessage) -> Vec<Reply> {
    let ClientMessage::Submit {
        request_id,
        command_id,
        ..
    } = frame
    else {
        panic!("unexpected frame {frame:?}");
    };
    vec![reply(&ServerMessage::CommandAccepted {
        request_id: *request_id,
        command_id: *command_id,
    })]
}

struct Running {
    commands: mpsc::UnboundedSender<Command>,
    updates: mpsc::UnboundedReceiver<Update>,
    task_id: TaskId,
}

/// Starts a session at `endpoints` and follows one chat in it.
fn start(endpoints: Endpoints) -> Running {
    let (commands, received) = mpsc::unbounded_channel();
    let (sender, updates) = mpsc::unbounded_channel();
    let credentials = DeviceCredentials {
        device_id: DeviceId::new(),
        credential: DeviceCredential::new("credential"),
    };
    let key = Arc::new(MacKey::generate().expect("key"));
    let notice_key = Arc::new(NoticeSecret::generate().expect("notice key"));
    tokio::spawn(session(
        endpoints,
        credentials,
        key,
        notice_key,
        QUICK,
        received,
        sender,
    ));
    let task_id = TaskId::new();
    commands
        .send(Command::Follow {
            task_id,
            node_id: NodeId::new(),
            target: TargetRef::new("renoa"),
            after: None,
        })
        .expect("follow");
    Running {
        commands,
        updates,
        task_id,
    }
}

/// Waits until `heard` holds `count` frames that `wanted` picks.
async fn hear(
    heard: &Heard,
    count: usize,
    wanted: impl Fn(&ClientMessage) -> bool,
) -> Vec<(usize, ClientMessage)> {
    timeout(Duration::from_secs(10), async {
        loop {
            let picked: Vec<_> = heard
                .lock()
                .expect("heard")
                .iter()
                .filter(|(_, frame)| wanted(frame))
                .cloned()
                .collect();
            if picked.len() >= count {
                return picked;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the coordinator hears it")
}

fn is_submit(frame: &ClientMessage) -> bool {
    matches!(frame, ClientMessage::Submit { .. })
}

fn not_sent(updates: &mut mpsc::UnboundedReceiver<Update>) -> Vec<(TaskId, String)> {
    let mut refused = Vec::new();
    while let Ok(update) = updates.try_recv() {
        if let Update::NotSent { task_id, text } = update {
            refused.push((task_id, text));
        }
    }
    refused
}

#[tokio::test]
async fn a_message_cut_off_by_a_broken_connection_is_sent_again_under_the_same_id() {
    let (endpoints, heard) = coordinator(Arc::new(|connection, frame: &ClientMessage| {
        chat_replies(frame).unwrap_or_else(|| {
            // The first connection breaks with the message in flight.
            if connection == 0 {
                vec![GARBAGE.to_owned()]
            } else {
                accepted(frame)
            }
        })
    }))
    .await;
    let mut running = start(endpoints);
    hear(&heard, 1, |frame| {
        matches!(frame, ClientMessage::Attach { .. })
    })
    .await;
    running
        .commands
        .send(Command::Send {
            task_id: running.task_id,
            text: "Make it vegetarian".to_owned(),
            steer: true,
            sent: None,
        })
        .expect("send");

    let submits = hear(&heard, 2, is_submit).await;
    let sent: Vec<_> = submits
        .iter()
        .map(|(connection, frame)| match frame {
            ClientMessage::Submit {
                task_id,
                command_id,
                input,
                ..
            } => (*connection, *task_id, *command_id, input.clone()),
            other => panic!("unexpected frame {other:?}"),
        })
        .collect();
    assert_eq!(
        (sent[0].0, sent[1].0),
        (0, 1),
        "sent again on the next connection"
    );
    assert_eq!(sent[0].1, running.task_id);
    assert_eq!(sent[0].1, sent[1].1);
    assert_eq!(sent[0].2, sent[1].2, "one id, so Renoa admits it once");
    assert!(
        sent[0].3.steer(),
        "a steer goes out as one: {:?}",
        sent[0].3
    );
    assert_eq!(sent[0].3.text(), "Make it vegetarian");
    assert_eq!(sent[0].3, sent[1].3);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(hear(&heard, 2, is_submit).await.len(), 2, "sent no more");
    assert!(
        not_sent(&mut running.updates).is_empty(),
        "nothing came back"
    );
}

#[tokio::test]
async fn a_refused_message_comes_back_and_the_connection_stays() {
    let (endpoints, heard) = coordinator(Arc::new(|_, frame: &ClientMessage| {
        chat_replies(frame).unwrap_or_else(|| {
            let ClientMessage::Submit { request_id, .. } = frame else {
                panic!("unexpected frame {frame:?}");
            };
            vec![reply(&ServerMessage::Error {
                request_id: Some(*request_id),
                code: ErrorCode::NodeOffline,
                message: "offline".to_owned(),
            })]
        })
    }))
    .await;
    let mut running = start(endpoints);
    running
        .commands
        .send(Command::Send {
            task_id: running.task_id,
            text: "Hello".to_owned(),
            steer: false,
            sent: None,
        })
        .expect("send");
    hear(&heard, 1, is_submit).await;
    let refused = timeout(Duration::from_secs(10), async {
        loop {
            let refused = not_sent(&mut running.updates);
            if !refused.is_empty() {
                return refused;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the refusal comes back");
    assert_eq!(refused, [(running.task_id, "Hello".to_owned())]);
    let connections = heard.lock().expect("heard").iter().map(|(n, _)| *n).max();
    assert_eq!(connections, Some(0), "a refusal is not a lost connection");
}

#[tokio::test]
async fn a_long_replay_does_not_hold_up_the_reply_it_came_with() {
    const REPLAYED: u64 = 600;
    let (endpoints, heard) = coordinator(Arc::new(|_, frame: &ClientMessage| match frame {
        ClientMessage::Attach {
            request_id,
            task_id,
            ..
        } => {
            // More events than the client buffers, all before the reply.
            let mut replies: Vec<Reply> = (0..REPLAYED)
                .map(|sequence| {
                    reply(&ServerMessage::TaskEvent {
                        event: said(*task_id, sequence, CommandInput::from_text("again")),
                    })
                })
                .collect();
            replies.push(reply(&ServerMessage::Attached {
                request_id: *request_id,
                task_id: *task_id,
                through_sequence: Some(REPLAYED - 1),
            }));
            replies
        }
        ClientMessage::Submit { .. } => accepted(frame),
        other => chat_replies(other).expect("a chat frame"),
    }))
    .await;
    let mut running = start(endpoints);
    hear(&heard, 1, |frame| {
        matches!(frame, ClientMessage::Attach { .. })
    })
    .await;
    running
        .commands
        .send(Command::Send {
            task_id: running.task_id,
            text: "Hello".to_owned(),
            steer: false,
            sent: None,
        })
        .expect("send");
    let submits = hear(&heard, 1, is_submit).await;
    assert_eq!(
        submits[0].0, 0,
        "the call after the replay ran on the same connection"
    );
    let mut events = 0;
    timeout(Duration::from_secs(10), async {
        while events < REPLAYED {
            if let Some(Update::Event(_)) = running.updates.recv().await {
                events += 1;
            }
        }
    })
    .await
    .expect("every replayed event reaches the views");
}

#[tokio::test]
async fn the_main_agent_and_each_agents_purpose_and_look_are_read_until_the_host_answers() {
    const PLANS: &str = "1b4e28ba-2fa1-11d2-883f-0016d3cca427";
    const GONE: &str = "6fa459ea-ee8a-3ca4-894e-db77e160355e";
    const LATE: &str = "886313e1-3b8a-5372-9b90-0c9aee199e5d";
    let late_asked = Arc::new(AtomicUsize::new(0));
    let asked = Arc::clone(&late_asked);
    let node_id = NodeId::new();
    let (endpoints, heard) = coordinator(Arc::new(move |_, frame: &ClientMessage| match frame {
        ClientMessage::ListTargets { request_id } => vec![reply(&ServerMessage::TargetList {
            request_id: *request_id,
            targets: [PLANS, GONE, LATE]
                .map(|id| format!("agent:{id}"))
                .into_iter()
                .chain(["renoa".to_owned()])
                .map(|target| TargetSummary {
                    node_id,
                    target: TargetRef::new(target),
                    name: "Agent".to_owned(),
                })
                .collect(),
        })],
        ClientMessage::CallHost {
            request_id, path, ..
        } => {
            let answer = |status: u16, body: &str| ServerMessage::HostAnswer {
                request_id: *request_id,
                status,
                body: body.to_owned(),
            };
            let definition = |purpose: &str| {
                answer(
                    200,
                    &format!(r#"{{"id":"x","operational":{{"purpose":"{purpose}"}}}}"#),
                )
            };
            let plans = format!("/v1/host/agents/{PLANS}");
            let late = format!("/v1/host/agents/{LATE}");
            vec![reply(&match path.as_str() {
                "/v1/host" => answer(
                    200,
                    &format!(r#"{{"automations":[],"main_agent":{{"main_agent_id":"{PLANS}"}}}}"#),
                ),
                path if path == plans => definition("Plans dinners"),
                path if path == format!("{plans}/appearance") => answer(200, LOOK),
                path if path == format!("/v1/host/agents/{GONE}") => answer(404, "{}"),
                path if path == late && asked.fetch_add(1, Ordering::SeqCst) == 0 => {
                    ServerMessage::Error {
                        request_id: Some(*request_id),
                        code: ErrorCode::NodeOffline,
                        message: "Your Host could not be reached.".to_owned(),
                    }
                }
                path if path == late => definition(r"  Books trips\n"),
                path if path == format!("{late}/appearance") => answer(404, "{}"),
                other => panic!("asked for {other}"),
            })]
        }
        other => chat_replies(other).expect("a chat frame"),
    }))
    .await;
    let mut running = start(endpoints);

    let mut told = Vec::new();
    timeout(Duration::from_secs(10), async {
        while told.len() < 4 {
            match running.updates.recv().await {
                Some(Update::Main(main)) => told.push(format!("main {main:?}")),
                Some(Update::Purpose {
                    node_id: of,
                    target,
                    purpose,
                }) => {
                    assert_eq!(of, node_id);
                    told.push(format!("{target} is for {purpose}"));
                }
                Some(Update::Look {
                    node_id: of,
                    target,
                    look,
                }) => {
                    assert_eq!(of, node_id);
                    assert_eq!(look.hair_front, HairFront::SidePart);
                    assert!(look.tuft);
                    told.push(format!("{target} has a look"));
                }
                _ => {}
            }
        }
    })
    .await
    .expect("the main agent, both purposes and the look arrive");
    assert_eq!(
        told,
        [
            format!("main Some({PLANS})"),
            format!("agent:{PLANS} is for Plans dinners"),
            format!("agent:{PLANS} has a look"),
            format!("agent:{LATE} is for Books trips"),
        ]
    );

    // Several more agent lists later, only the main agent is asked again,
    // and an unchanged one is not told again.
    hear(&heard, 6, |frame| {
        matches!(frame, ClientMessage::ListTargets { .. })
    })
    .await;
    while let Ok(update) = running.updates.try_recv() {
        assert!(!matches!(
            update,
            Update::Main(_) | Update::Purpose { .. } | Update::Look { .. }
        ));
    }
    let calls: Vec<_> = heard
        .lock()
        .expect("heard")
        .iter()
        .filter_map(|(_, frame)| match frame {
            ClientMessage::CallHost {
                method, path, body, ..
            } => Some((*method, path.clone(), body.clone())),
            _ => None,
        })
        .filter(|(_, path, _)| path != "/v1/host")
        .collect();
    let path = |path: String| (HostMethod::Get, path, None);
    let agent = |id: &str| path(format!("/v1/host/agents/{id}"));
    let look = |id: &str| path(format!("/v1/host/agents/{id}/appearance"));
    assert_eq!(
        calls,
        [
            agent(PLANS),
            look(PLANS),
            // No such agent has no look to ask for.
            agent(GONE),
            // A purpose not read yet holds back the look.
            agent(LATE),
            agent(LATE),
            look(LATE),
        ]
    );
    let connections = heard.lock().expect("heard").iter().map(|(n, _)| *n).max();
    assert_eq!(
        connections,
        Some(0),
        "a Host's refusal is not a lost connection"
    );
}

/// A look as the Host sends it, with names of more than one word.
const LOOK: &str = r#"{"look":{"hair_front":"side_part","hair_back":"twin_tails","hair_colour":"auburn","skin":"deep","eyes":"cat","cheeks":"freckles","wear":"cat_ears","tile":"ochre","tuft":true},"picture_revision":null}"#;

#[tokio::test]
async fn a_host_that_never_answers_holds_up_no_message() {
    let (endpoints, heard) = coordinator(Arc::new(|_, frame: &ClientMessage| match frame {
        ClientMessage::ListTargets { request_id } => vec![reply(&ServerMessage::TargetList {
            request_id: *request_id,
            targets: vec![TargetSummary {
                node_id: NodeId::new(),
                target: TargetRef::new("agent:1b4e28ba-2fa1-11d2-883f-0016d3cca427"),
                name: "Chef".to_owned(),
            }],
        })],
        ClientMessage::CallHost { .. } => Vec::new(),
        ClientMessage::Submit { .. } => accepted(frame),
        other => chat_replies(other).expect("a chat frame"),
    }))
    .await;
    let mut running = start(endpoints);
    hear(&heard, 1, |frame| {
        matches!(frame, ClientMessage::CallHost { .. })
    })
    .await;
    running
        .commands
        .send(Command::Send {
            task_id: running.task_id,
            text: "Still there?".to_owned(),
            steer: false,
            sent: None,
        })
        .expect("send");
    hear(&heard, 1, is_submit).await;
    hear(&heard, 4, |frame| {
        matches!(frame, ClientMessage::ListTargets { .. })
    })
    .await;
    let calls = hear(&heard, 1, |frame| {
        matches!(frame, ClientMessage::CallHost { .. })
    })
    .await;
    assert_eq!(calls.len(), 1, "one unanswered call at a time");
    while let Ok(update) = running.updates.try_recv() {
        assert!(
            !matches!(update, Update::Linked(false)),
            "the link stays up"
        );
    }
}

#[tokio::test]
async fn a_host_call_while_renoa_is_out_of_reach_is_answered_lost_and_the_session_ends_with_the_views()
 {
    // A port nothing listens on: every connection fails.
    let closed = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let endpoints = Endpoints::at(&format!("http://{}", closed.local_addr().expect("address")));
    drop(closed);
    let (commands, received) = mpsc::unbounded_channel();
    let (sender, mut updates) = mpsc::unbounded_channel();
    let credentials = DeviceCredentials {
        device_id: DeviceId::new(),
        credential: DeviceCredential::new("credential"),
    };
    let key = Arc::new(MacKey::generate().expect("key"));
    let notice_key = Arc::new(NoticeSecret::generate().expect("notice key"));
    let running = tokio::spawn(session(
        endpoints,
        credentials,
        key,
        notice_key,
        QUICK,
        received,
        sender,
    ));
    let Some(Update::Linked(false)) = timeout(Duration::from_secs(5), updates.recv())
        .await
        .expect("the session says it is offline")
    else {
        panic!("the first update is the failed link");
    };
    let (answered, answer) = oneshot::channel();
    commands
        .send(Command::Host(HostCall {
            method: HostMethod::Get,
            path: "/v1/host".to_owned(),
            body: None,
            answered,
        }))
        .expect("call");
    let answer = timeout(Duration::from_secs(5), answer)
        .await
        .expect("answered while offline")
        .expect("an answer");
    assert_eq!(answer, Answer::Lost);
    drop(commands);
    timeout(Duration::from_secs(5), running)
        .await
        .expect("the session ends once no view is left")
        .expect("the session ran");
}

#[tokio::test]
async fn a_sender_that_asks_hears_whether_its_message_was_taken_and_the_chat_does_not() {
    let (endpoints, _heard) = coordinator(Arc::new(|_, frame: &ClientMessage| {
        chat_replies(frame).unwrap_or_else(|| match frame {
            ClientMessage::Submit {
                request_id, input, ..
            } if input.text() == "No" => vec![reply(&ServerMessage::Error {
                request_id: Some(*request_id),
                code: ErrorCode::NodeOffline,
                message: "offline".to_owned(),
            })],
            _ => accepted(frame),
        })
    }))
    .await;
    let mut running = start(endpoints);
    let send = |text: &str| {
        let (sent, heard) = oneshot::channel();
        running
            .commands
            .send(Command::Send {
                task_id: running.task_id,
                text: text.to_owned(),
                steer: false,
                sent: Some(sent),
            })
            .expect("send");
        heard
    };
    let (taken, refused) = (send("Yes"), send("No"));
    let within = Duration::from_secs(10);
    assert_eq!(timeout(within, taken).await.expect("heard"), Ok(true));
    assert_eq!(timeout(within, refused).await.expect("heard"), Ok(false));
    assert!(
        not_sent(&mut running.updates).is_empty(),
        "the sender shows its own refusal, not the chat's composer"
    );
}

#[tokio::test]
async fn a_share_link_request_resolves_against_endpoints_origin_or_none_on_refusal_or_offline() {
    let found = Uuid::new_v4();
    let missing = Uuid::new_v4();
    let untrusted = Uuid::new_v4();
    let (endpoints, heard) = coordinator(Arc::new(move |_, frame: &ClientMessage| {
        chat_replies(frame).unwrap_or_else(|| match frame {
            ClientMessage::ShareLink {
                request_id,
                share_id,
            } if *share_id == found => vec![reply(&ServerMessage::ShareLinkIssued {
                request_id: *request_id,
                path: ShareLink::new(format!("/s/{share_id}?ticket=secret-ticket")),
            })],
            ClientMessage::ShareLink {
                request_id,
                share_id,
            } if *share_id == untrusted => vec![reply(&ServerMessage::ShareLinkIssued {
                request_id: *request_id,
                path: ShareLink::new("//evil.example/phish".to_owned()),
            })],
            ClientMessage::ShareLink { request_id, .. } => vec![reply(&ServerMessage::Error {
                request_id: Some(*request_id),
                code: ErrorCode::NotFound,
                message: "gone".to_owned(),
            })],
            other => panic!("unexpected frame {other:?}"),
        })
    }))
    .await;
    let origin = endpoints.origin.clone();
    let mut running = start(endpoints);
    hear(&heard, 1, |frame| {
        matches!(frame, ClientMessage::Attach { .. })
    })
    .await;

    let (tx, rx) = oneshot::channel();
    running
        .commands
        .send(Command::ShareLink {
            share_id: found,
            answered: tx,
        })
        .expect("send share link");
    let url = timeout(Duration::from_secs(5), rx)
        .await
        .expect("answered")
        .expect("sender");
    assert_eq!(
        url,
        Some(format!("{origin}/s/{found}?ticket=secret-ticket"))
    );

    for other in [missing, untrusted] {
        let (tx, rx) = oneshot::channel();
        running
            .commands
            .send(Command::ShareLink {
                share_id: other,
                answered: tx,
            })
            .expect("send share link");
        assert_eq!(
            timeout(Duration::from_secs(5), rx)
                .await
                .expect("answered")
                .expect("sender"),
            None
        );
    }

    let connections = heard.lock().expect("heard").iter().map(|(n, _)| *n).max();
    assert_eq!(
        connections,
        Some(0),
        "a refused share link does not drop the connection"
    );
    drop(running.commands);
    while running.updates.try_recv().is_ok() {}

    // While offline, ShareLink answers None immediately without queueing.
    let closed = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let offline_endpoints =
        Endpoints::at(&format!("http://{}", closed.local_addr().expect("address")));
    drop(closed);
    let (commands, received) = mpsc::unbounded_channel();
    let (sender, mut updates) = mpsc::unbounded_channel();
    let credentials = DeviceCredentials {
        device_id: DeviceId::new(),
        credential: DeviceCredential::new("credential"),
    };
    let key = Arc::new(MacKey::generate().expect("key"));
    let notice_key = Arc::new(NoticeSecret::generate().expect("notice key"));
    let task = tokio::spawn(session(
        offline_endpoints,
        credentials,
        key,
        notice_key,
        QUICK,
        received,
        sender,
    ));
    let Some(Update::Linked(false)) = timeout(Duration::from_secs(5), updates.recv())
        .await
        .expect("offline status")
    else {
        panic!("expected offline update");
    };
    let (tx, rx) = oneshot::channel();
    commands
        .send(Command::ShareLink {
            share_id: found,
            answered: tx,
        })
        .expect("send share link offline");
    assert_eq!(
        timeout(Duration::from_secs(5), rx)
            .await
            .expect("answered offline")
            .expect("sender"),
        None
    );
    drop(commands);
    let _ = timeout(Duration::from_secs(5), task).await;
}

#[tokio::test]
async fn stop_command_forwards_stop_outcomes_refusal_and_offline_uncertainty() {
    let running_cmd = CommandId::new();
    let ended_cmd = CommandId::new();
    let refused_cmd = CommandId::new();
    let (endpoints, heard) = coordinator(Arc::new(move |_, frame| {
        if let Some(replies) = chat_replies(frame) {
            return replies;
        }
        let ClientMessage::StopCommand {
            request_id,
            command_id,
            ..
        } = frame
        else {
            panic!("unexpected frame {frame:?}");
        };
        if *command_id == running_cmd {
            vec![reply(&ServerMessage::StopAccepted {
                request_id: *request_id,
                command_id: *command_id,
                outcome: StopOutcome::Requested,
            })]
        } else if *command_id == ended_cmd {
            vec![reply(&ServerMessage::StopAccepted {
                request_id: *request_id,
                command_id: *command_id,
                outcome: StopOutcome::AlreadyEnded,
            })]
        } else {
            vec![reply(&ServerMessage::Error {
                request_id: Some(*request_id),
                code: ErrorCode::NotFound,
                message: "refused".to_owned(),
            })]
        }
    }))
    .await;

    let mut running = start(endpoints);
    while running
        .updates
        .recv()
        .await
        .is_some_and(|u| !matches!(u, Update::Linked(true)))
    {}

    let (tx, rx) = oneshot::channel();
    running
        .commands
        .send(Command::Stop {
            task_id: running.task_id,
            command_id: running_cmd,
            answered: tx,
        })
        .expect("send stop");
    assert_eq!(
        timeout(Duration::from_secs(5), rx)
            .await
            .expect("stop answer")
            .expect("sender"),
        Ok(StopOutcome::Requested)
    );

    let (tx, rx) = oneshot::channel();
    running
        .commands
        .send(Command::Stop {
            task_id: running.task_id,
            command_id: ended_cmd,
            answered: tx,
        })
        .expect("send stop ended");
    assert_eq!(
        timeout(Duration::from_secs(5), rx)
            .await
            .expect("ended answer")
            .expect("sender"),
        Ok(StopOutcome::AlreadyEnded)
    );

    let (tx, rx) = oneshot::channel();
    running
        .commands
        .send(Command::Stop {
            task_id: running.task_id,
            command_id: refused_cmd,
            answered: tx,
        })
        .expect("send stop refused");
    assert_eq!(
        timeout(Duration::from_secs(5), rx)
            .await
            .expect("refused answer")
            .expect("sender"),
        Err(StopProblem::Refused)
    );

    let connections = heard.lock().expect("heard").iter().map(|(n, _)| *n).max();
    assert_eq!(
        connections,
        Some(0),
        "a refused stop does not drop the connection"
    );
    drop(running.commands);

    let closed = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let offline_endpoints =
        Endpoints::at(&format!("http://{}", closed.local_addr().expect("address")));
    drop(closed);
    let (commands, received) = mpsc::unbounded_channel();
    let (sender, mut updates) = mpsc::unbounded_channel();
    let credentials = DeviceCredentials {
        device_id: DeviceId::new(),
        credential: DeviceCredential::new("credential"),
    };
    let key = Arc::new(MacKey::generate().expect("key"));
    let notice_key = Arc::new(NoticeSecret::generate().expect("notice key"));
    let task = tokio::spawn(session(
        offline_endpoints,
        credentials,
        key,
        notice_key,
        QUICK,
        received,
        sender,
    ));
    let Some(Update::Linked(false)) = timeout(Duration::from_secs(5), updates.recv())
        .await
        .expect("offline status")
    else {
        panic!("expected offline update");
    };
    let (tx, rx) = oneshot::channel();
    commands
        .send(Command::Stop {
            task_id: running.task_id,
            command_id: running_cmd,
            answered: tx,
        })
        .expect("send stop offline");
    assert_eq!(
        timeout(Duration::from_secs(5), rx)
            .await
            .expect("answered offline")
            .expect("sender"),
        Err(StopProblem::Uncertain)
    );
    drop(commands);
    let _ = timeout(Duration::from_secs(5), task).await;
}

#[tokio::test]
async fn setup_notices_register_key_open_valid_ack_unreadable_refresh_on_turn_end_and_ack_on_command()
 {
    let device_id = DeviceId::new();
    let notice_key = Arc::new(NoticeSecret::generate().expect("notice key"));
    let point = notice_key.public_key().point().expect("point");
    let valid_id = Uuid::new_v4();
    let expired_id = Uuid::new_v4();
    let corrupt_id = Uuid::new_v4();
    let later_id = Uuid::new_v4();
    let future = now_ms() + 300_000;
    let past = now_ms() - 60_000;

    let valid_sealed = seal_for_test(
        &point,
        valid_id,
        device_id,
        serde_json::json!({
            "kind": "credentials",
            "label": "Google OAuth",
            "url": "https://renoa.live/setup#v=1&key=abc",
            "expires_at_ms": future,
        })
        .to_string()
        .as_bytes(),
    );
    let expired_sealed = seal_for_test(
        &point,
        expired_id,
        device_id,
        serde_json::json!({
            "kind": "authorization",
            "label": "Old",
            "url": "https://accounts.google.com/auth",
            "expires_at_ms": past,
        })
        .to_string()
        .as_bytes(),
    );
    let later_sealed = seal_for_test(
        &point,
        later_id,
        device_id,
        serde_json::json!({
            "kind": "authorization",
            "label": "Gmail",
            "url": "https://accounts.google.com/o/oauth2/auth",
            "expires_at_ms": future,
        })
        .to_string()
        .as_bytes(),
    );

    let phase = Arc::new(AtomicUsize::new(0));
    let phase_in = Arc::clone(&phase);
    let (endpoints, heard) = coordinator(Arc::new(move |_, frame: &ClientMessage| match frame {
        ClientMessage::ListNotices { request_id } => {
            let notices = match phase_in.load(Ordering::SeqCst) {
                0 => vec![
                    HeldNotice {
                        notice_id: valid_id,
                        sealed: valid_sealed.clone(),
                        expires_at_ms: future,
                    },
                    HeldNotice {
                        notice_id: expired_id,
                        sealed: expired_sealed.clone(),
                        expires_at_ms: past,
                    },
                    HeldNotice {
                        notice_id: corrupt_id,
                        sealed: "not-valid-ciphertext".to_owned(),
                        expires_at_ms: future,
                    },
                ],
                1 => Vec::new(),
                _ => vec![HeldNotice {
                    notice_id: later_id,
                    sealed: later_sealed.clone(),
                    expires_at_ms: future,
                }],
            };
            vec![reply(&ServerMessage::NoticeList {
                request_id: *request_id,
                notices,
            })]
        }
        ClientMessage::Attach {
            request_id,
            task_id,
            ..
        } => {
            // Delivering ExecutionTerminated wakes a fresh notice read.
            phase_in.store(2, Ordering::SeqCst);
            vec![
                reply(&ServerMessage::TaskEvent {
                    event: executed(
                        *task_id,
                        0,
                        ExecutionEventKind::ExecutionTerminated {
                            terminal: ExecutionTerminal::Completed,
                        },
                    ),
                }),
                reply(&ServerMessage::Attached {
                    request_id: *request_id,
                    task_id: *task_id,
                    through_sequence: Some(0),
                }),
            ]
        }
        other => chat_replies(other).expect("expected chat or notice frame"),
    }))
    .await;

    let (commands, received) = mpsc::unbounded_channel();
    let (sender, mut updates) = mpsc::unbounded_channel();
    let credentials = DeviceCredentials {
        device_id,
        credential: DeviceCredential::new("credential"),
    };
    let key = Arc::new(MacKey::generate().expect("key"));
    let expected_pub = notice_key.public_key();
    let session_task = tokio::spawn(session(
        endpoints,
        credentials,
        key,
        notice_key,
        Pace {
            collect_every: Duration::from_millis(5),
            retry_after: Duration::from_millis(5),
            // Long poll interval proves the turn-end event itself wakes the refresh.
            agents_every: Duration::from_secs(60),
        },
        received,
        sender,
    ));

    let registered = hear(&heard, 1, |frame| {
        matches!(frame, ClientMessage::SetNoticeKey { .. })
    })
    .await;
    assert!(matches!(
        &registered[0].1,
        ClientMessage::SetNoticeKey { public_key, .. } if *public_key == expected_pub
    ));

    let first_links = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(Update::SetupLinks(links)) = updates.recv().await {
                return links;
            }
        }
    })
    .await
    .expect("first setup links arrive");
    assert_eq!(first_links.len(), 1);
    assert_eq!(first_links[0].notice_id, valid_id);
    assert_eq!(first_links[0].link.kind, SetupLinkKind::Credentials);
    assert_eq!(first_links[0].link.label, "Google OAuth");

    // Corrupt notice was acknowledged automatically without user action.
    let acks = hear(&heard, 1, |frame| {
        matches!(
            frame,
            ClientMessage::AcknowledgeNotice { notice_id, .. } if *notice_id == corrupt_id
        )
    })
    .await;
    assert_eq!(acks.len(), 1);

    // Owner acknowledges the valid notice.
    phase.store(1, Ordering::SeqCst);
    let (tx, rx) = oneshot::channel();
    commands
        .send(Command::AcknowledgeNotice {
            notice_id: valid_id,
            answered: tx,
        })
        .expect("ack");
    assert_eq!(timeout(Duration::from_secs(5), rx).await.unwrap(), Ok(true));
    let cleared = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(Update::SetupLinks(links)) = updates.recv().await {
                return links;
            }
        }
    })
    .await
    .expect("cleared update");
    assert!(cleared.is_empty());

    // Following a chat whose event terminates a turn triggers a fresh read.
    commands
        .send(Command::Follow {
            task_id: TaskId::new(),
            node_id: NodeId::new(),
            target: TargetRef::new("renoa"),
            after: None,
        })
        .expect("follow");
    let second_links = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(Update::SetupLinks(links)) = updates.recv().await
                && !links.is_empty()
            {
                return links;
            }
        }
    })
    .await
    .expect("turn-end refresh delivers the new setup link");
    assert_eq!(second_links.len(), 1);
    assert_eq!(second_links[0].notice_id, later_id);
    assert_eq!(second_links[0].link.label, "Gmail");

    drop(commands);
    let _ = timeout(Duration::from_secs(5), session_task).await;
}

#[tokio::test]
async fn a_transient_set_notice_key_failure_retries_on_the_same_connection() {
    let device_id = DeviceId::new();
    let notice_key = Arc::new(NoticeSecret::generate().expect("notice key"));
    let point = notice_key.public_key().point().expect("point");
    let notice_id = Uuid::new_v4();
    let future = now_ms() + 300_000;
    let sealed = seal_for_test(
        &point,
        notice_id,
        device_id,
        serde_json::json!({
            "kind": "authorization",
            "label": "Gmail",
            "url": "https://accounts.google.com/o/oauth2/auth",
            "expires_at_ms": future,
        })
        .to_string()
        .as_bytes(),
    );

    let key_attempts = Arc::new(AtomicUsize::new(0));
    let key_attempts_in = Arc::clone(&key_attempts);
    let (endpoints, heard) = coordinator(Arc::new(move |_, frame: &ClientMessage| match frame {
        ClientMessage::SetNoticeKey { request_id, .. } => {
            if key_attempts_in.fetch_add(1, Ordering::SeqCst) == 0 {
                vec![reply(&ServerMessage::Error {
                    request_id: Some(*request_id),
                    code: ErrorCode::Internal,
                    message: "temporary".to_owned(),
                })]
            } else {
                vec![reply(&ServerMessage::NoticeKeySet {
                    request_id: *request_id,
                })]
            }
        }
        ClientMessage::ListNotices { request_id } => vec![reply(&ServerMessage::NoticeList {
            request_id: *request_id,
            notices: vec![HeldNotice {
                notice_id,
                sealed: sealed.clone(),
                expires_at_ms: future,
            }],
        })],
        other => chat_replies(other).expect("expected frame"),
    }))
    .await;

    let (commands, received) = mpsc::unbounded_channel();
    let (sender, mut updates) = mpsc::unbounded_channel();
    let credentials = DeviceCredentials {
        device_id,
        credential: DeviceCredential::new("credential"),
    };
    let key = Arc::new(MacKey::generate().expect("key"));
    let expected_pub = notice_key.public_key();
    let session_task = tokio::spawn(session(
        endpoints,
        credentials,
        key,
        notice_key,
        Pace {
            collect_every: Duration::from_millis(5),
            retry_after: Duration::from_millis(5),
            agents_every: Duration::from_millis(30),
        },
        received,
        sender,
    ));

    let registered = hear(&heard, 2, |frame| {
        matches!(frame, ClientMessage::SetNoticeKey { .. })
    })
    .await;
    assert_eq!(
        (registered[0].0, registered[1].0),
        (0, 0),
        "retries SetNoticeKey on the same connection"
    );
    assert!(matches!(
        &registered[1].1,
        ClientMessage::SetNoticeKey { public_key, .. } if *public_key == expected_pub
    ));

    let links = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(Update::SetupLinks(links)) = updates.recv().await
                && !links.is_empty()
            {
                return links;
            }
        }
    })
    .await
    .expect("notice arrives once registration succeeds");
    assert_eq!(links[0].notice_id, notice_id);

    drop(commands);
    let _ = timeout(Duration::from_secs(5), session_task).await;
}

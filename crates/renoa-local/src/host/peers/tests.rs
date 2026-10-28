use std::sync::{Arc, Mutex};

use renoa_agent::{BoxFuture, ContentBlock, ToolCall, ToolErrorCode, ToolOutput};
use renoa_kernel::{AgentId, CommandId};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{
    AgentPeers, CORRECT_CALL, Message, PeerAgent, PeerError, PeerMessage, Roster, bindings,
};
use crate::plugins::host::HostPluginId;

struct Fake {
    roster: Vec<PeerAgent>,
    send: Result<(), PeerError>,
    /// Never finishes sending, so only cancellation ends the call.
    hang: bool,
    sent: Mutex<Vec<PeerMessage>>,
}

impl Fake {
    fn new(names: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            roster: names.iter().map(|name| peer(name)).collect(),
            send: Ok(()),
            hang: false,
            sent: Mutex::new(Vec::new()),
        })
    }

    fn sent(&self) -> Vec<PeerMessage> {
        self.sent.lock().expect("sent messages").clone()
    }
}

impl AgentPeers for Fake {
    fn roster(&self, _: AgentId) -> BoxFuture<'_, Result<Vec<PeerAgent>, PeerError>> {
        Box::pin(async move { Ok(self.roster.clone()) })
    }

    fn send(&self, message: PeerMessage) -> BoxFuture<'_, Result<(), PeerError>> {
        Box::pin(async move {
            if self.hang {
                std::future::pending::<()>().await;
            }
            self.sent.lock().expect("sent messages").push(message);
            self.send.clone()
        })
    }
}

fn peer(name: &str) -> PeerAgent {
    PeerAgent {
        name: name.to_owned(),
        route: format!("secret-route-for-{name}"),
    }
}

fn call(id: &str, arguments: Value) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        name: "message_agent".to_owned(),
        arguments,
        thought_signature: None,
        namespace: None,
    }
}

fn message_tool(peers: &Arc<Fake>, command: Option<CommandId>) -> Message {
    let [_, binding] = bindings(
        Arc::clone(peers) as Arc<dyn AgentPeers>,
        AgentId::new(),
        command,
    );
    assert_eq!(binding.tool_name(), "message_agent");
    Message {
        peers: Arc::clone(peers) as Arc<dyn AgentPeers>,
        actor: AgentId::new(),
        command,
        spec: binding.tool().spec().clone(),
    }
}

fn roster_tool(peers: &Arc<Fake>) -> Roster {
    let [binding, _] = bindings(
        Arc::clone(peers) as Arc<dyn AgentPeers>,
        AgentId::new(),
        None,
    );
    assert_eq!(binding.tool_name(), "agent_roster");
    Roster {
        peers: Arc::clone(peers) as Arc<dyn AgentPeers>,
        actor: AgentId::new(),
        spec: binding.tool().spec().clone(),
    }
}

fn output(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            ContentBlock::Image { .. } => None,
        })
        .collect()
}

async fn send(
    tool: &Message,
    id: &str,
    arguments: Value,
) -> Result<ToolOutput, renoa_agent::ToolError> {
    tool.run(call(id, arguments), CancellationToken::new())
        .await
}

#[test]
fn both_tools_are_small_and_belong_to_the_agents_plugin() {
    let peers = Fake::new(&[]);
    let message = message_tool(&peers, None);
    let schema = &message.spec.input_schema;
    assert_eq!(schema["required"], json!(["to", "text"]));
    assert_eq!(schema["additionalProperties"], json!(false));
    assert_eq!(
        schema["properties"].as_object().expect("properties").len(),
        2
    );
    assert_eq!(
        roster_tool(&peers).spec.input_schema["properties"],
        json!({})
    );
    for name in [
        crate::capabilities::AGENT_ROSTER,
        crate::capabilities::MESSAGE_AGENT,
    ] {
        assert_eq!(
            HostPluginId::owner(name),
            Some(HostPluginId::Agents),
            "{name} is governed by the Agents switch"
        );
    }
}

#[tokio::test]
async fn a_message_names_the_sender_by_its_command_and_is_stable_per_call() {
    let command = CommandId::new();
    let peers = Fake::new(&["Desk", "Research"]);
    let tool = message_tool(&peers, Some(command));

    let done = send(
        &tool,
        "call-1",
        json!({"to": " research ", "text": "Summarise."}),
    )
    .await
    .expect("a message to a listed agent is sent");
    assert!(
        output(&done).starts_with("Sent to Research."),
        "{}",
        output(&done)
    );
    assert!(!output(&done).contains("secret-route"));

    send(
        &tool,
        "call-1",
        json!({"to": "Research", "text": "Summarise."}),
    )
    .await
    .expect("the same call again");
    send(
        &tool,
        "call-2",
        json!({"to": "Research", "text": "Summarise."}),
    )
    .await
    .expect("another call");
    let sent = peers.sent();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[0].from_command.to_string(), command.to_string());
    assert_eq!(sent[0].to, peer("Research"));
    assert_eq!(sent[0].text, "Summarise.");
    assert_eq!(
        sent[0].message_id, sent[1].message_id,
        "a retried call keeps its identity"
    );
    assert_ne!(
        sent[0].message_id, sent[2].message_id,
        "another call is another message"
    );

    let other = message_tool(&peers, Some(CommandId::new()));
    send(
        &other,
        "call-1",
        json!({"to": "Research", "text": "Summarise."}),
    )
    .await
    .expect("the same call id under another command");
    assert_ne!(peers.sent()[3].message_id, sent[0].message_id);
}

#[tokio::test]
async fn a_wrong_call_is_told_the_field_the_value_and_a_correct_call() {
    let peers = Fake::new(&["Desk", "Research"]);
    let tool = message_tool(&peers, Some(CommandId::new()));
    let cases = [
        (json!([1]), "received an array"),
        (json!({"text": "Hi."}), "`to` is missing"),
        (json!({"to": "Research"}), "`text` is missing"),
        (
            json!({"to": 7, "text": "Hi."}),
            "`to` must be a string but received a number",
        ),
        (
            json!({"to": "Research", "text": null}),
            "`text` must be a string but received null",
        ),
        (json!({"to": "Research", "text": "  "}), "`text` is empty"),
        (
            json!({"to": "Research", "text": "Hi.", "urgent": true}),
            "`urgent` is not a field",
        ),
    ];
    for (arguments, expected) in cases {
        let error = send(&tool, "call", arguments.clone())
            .await
            .expect_err("a malformed call is refused");
        assert_eq!(error.code(), ToolErrorCode::InvalidInput, "{arguments}");
        let message = error.to_string();
        assert!(message.contains(expected), "{arguments}: {message}");
        assert!(message.contains(CORRECT_CALL), "{arguments}: {message}");
        assert!(!error.outcome_is_unknown());
    }

    let missing = send(&tool, "call", json!({"to": "Nobody", "text": "Hi."}))
        .await
        .expect_err("an unknown name is refused");
    assert_eq!(missing.code(), ToolErrorCode::InvalidInput);
    let message = missing.to_string();
    assert!(message.contains("\"Nobody\""), "{message}");
    assert!(message.contains("\"Desk\", \"Research\""), "{message}");
    assert!(
        message.contains(r#"{"to": "Desk", "text": "..."}"#),
        "{message}"
    );
    assert!(peers.sent().is_empty(), "no refused call sent anything");
}

#[tokio::test]
async fn an_echoed_name_is_cut_short() {
    let peers = Fake::new(&["Desk"]);
    let tool = message_tool(&peers, Some(CommandId::new()));
    let long = "n".repeat(5_000);
    for arguments in [
        json!({"to": long, "text": "Hi."}),
        json!({"to": "Desk", "text": "Hi.", long.clone(): 1}),
    ] {
        let message = send(&tool, "call", arguments)
            .await
            .expect_err("refused")
            .to_string();
        assert!(message.len() < 1_000, "{} bytes", message.len());
    }
}

#[tokio::test]
async fn an_empty_or_ambiguous_roster_is_told_apart() {
    let nobody = Fake::new(&[]);
    let error = send(
        &message_tool(&nobody, Some(CommandId::new())),
        "call",
        json!({"to": "Desk", "text": "Hi."}),
    )
    .await
    .expect_err("nobody is on");
    assert_eq!(error.code(), ToolErrorCode::Unavailable);
    assert!(error.to_string().contains("Try again later"));

    let twins = Fake::new(&["Desk", "desk"]);
    let error = send(
        &message_tool(&twins, Some(CommandId::new())),
        "call",
        json!({"to": "DESK", "text": "Hi."}),
    )
    .await
    .expect_err("two agents share the name");
    assert_eq!(error.code(), ToolErrorCode::InvalidInput);
    assert!(error.to_string().contains("More than one agent"));
    assert!(twins.sent().is_empty());
}

#[tokio::test]
async fn a_message_outside_an_answer_has_nothing_to_speak_for() {
    let peers = Fake::new(&["Desk"]);
    let error = send(
        &message_tool(&peers, None),
        "call",
        json!({"to": "Desk", "text": "Hi."}),
    )
    .await
    .expect_err("no executing command");
    assert_eq!(error.code(), ToolErrorCode::Unavailable);
    assert!(error.to_string().contains("only while you are answering"));
    assert!(peers.sent().is_empty());
}

#[tokio::test]
async fn a_failed_delivery_says_whether_to_resend() {
    let refused = Arc::new(Fake {
        roster: vec![peer("Desk")],
        send: Err(PeerError::Refused("that message is too long".to_owned())),
        hang: false,
        sent: Mutex::new(Vec::new()),
    });
    let error = send(
        &message_tool(&refused, Some(CommandId::new())),
        "call",
        json!({"to": "Desk", "text": "Hi."}),
    )
    .await
    .expect_err("refused");
    assert_eq!(error.code(), ToolErrorCode::InvalidInput);
    assert!(
        error
            .to_string()
            .contains("Could not message Desk: that message is too long.")
    );
    // The agent reads that nothing was sent and that repeating the call as it
    // is will not help.
    assert!(error.to_string().contains("Nothing was sent."), "{error}");
    assert!(
        error.to_string().contains("would be refused again"),
        "{error}"
    );

    for (failure, advised) in [
        (
            PeerError::Unavailable("the connection is down".to_owned()),
            true,
        ),
        (
            PeerError::Unconfirmed("it may have arrived".to_owned()),
            false,
        ),
    ] {
        let reason = match &failure {
            PeerError::Unavailable(reason) | PeerError::Unconfirmed(reason) => reason.clone(),
            PeerError::Refused(_) => unreachable!("covered above"),
        };
        let peers = Arc::new(Fake {
            roster: vec![peer("Desk")],
            send: Err(failure),
            hang: false,
            sent: Mutex::new(Vec::new()),
        });
        let error = send(
            &message_tool(&peers, Some(CommandId::new())),
            "call",
            json!({"to": "Desk", "text": "Hi."}),
        )
        .await
        .expect_err("not delivered");
        // A turn is never halted over a message: the agent reads the text and
        // decides.
        assert_eq!(error.code(), ToolErrorCode::Unavailable);
        assert!(!error.outcome_is_unknown());
        assert!(error.to_string().contains(&reason));
        // An unavailable delivery says to try later; an unconfirmed one leaves
        // its own text to say what resending does.
        assert_eq!(
            error.to_string().contains("Trying again in a little while"),
            advised,
            "{error}"
        );
    }
}

/// A roster that cannot be fetched and a port that must never be asked to send.
struct NoRoster(PeerError);

impl AgentPeers for NoRoster {
    fn roster(&self, _: AgentId) -> BoxFuture<'_, Result<Vec<PeerAgent>, PeerError>> {
        Box::pin(async move { Err(self.0.clone()) })
    }

    fn send(&self, _: PeerMessage) -> BoxFuture<'_, Result<(), PeerError>> {
        Box::pin(async move { panic!("a failed lookup must not lead to a send") })
    }
}

#[tokio::test]
async fn a_failed_lookup_says_nothing_was_sent_and_whether_to_retry() {
    let command = CommandId::new();
    for (failure, retry) in [
        (
            PeerError::Unavailable("the list of agents could not be fetched".to_owned()),
            true,
        ),
        (
            PeerError::Refused("this machine has no owner".to_owned()),
            false,
        ),
    ] {
        let peers: Arc<dyn AgentPeers> = Arc::new(NoRoster(failure));
        let tool = Message {
            peers: Arc::clone(&peers),
            actor: AgentId::new(),
            command: Some(command),
            spec: bindings(Arc::clone(&peers), AgentId::new(), Some(command))[1]
                .tool()
                .spec()
                .clone(),
        };
        let error = tool
            .run(
                call("call", json!({"to": "Desk", "text": "Hi."})),
                CancellationToken::new(),
            )
            .await
            .expect_err("the lookup failed");
        let text = error.to_string();
        assert!(text.contains("so nothing was sent"), "{text}");
        assert!(!text.contains("delivered"), "{text}");
        assert_eq!(
            text.contains("Trying again in a little while may work."),
            retry,
            "{text}"
        );
        assert_eq!(text.contains("would be refused again"), !retry, "{text}");
    }
}

#[tokio::test]
async fn cancelling_a_send_reports_that_it_may_have_been_delivered() {
    let peers = Arc::new(Fake {
        roster: vec![peer("Desk")],
        send: Ok(()),
        hang: true,
        sent: Mutex::new(Vec::new()),
    });
    let tool = message_tool(&peers, Some(CommandId::new()));
    let cancellation = CancellationToken::new();
    let stop = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        stop.cancel();
    });
    let error = tool
        .run(
            call("call", json!({"to": "Desk", "text": "Hi."})),
            cancellation,
        )
        .await
        .expect_err("cancelled");
    assert_eq!(error.code(), ToolErrorCode::Cancelled);
    assert!(error.partial_changes_possible());
    assert!(error.to_string().contains("may or may not"));
}

#[tokio::test]
async fn the_roster_lists_names_only_and_takes_no_fields() {
    let peers = Fake::new(&["Desk", "Research"]);
    let tool = roster_tool(&peers);
    let listed = tool
        .run(call("call", json!({})), CancellationToken::new())
        .await
        .expect("the roster lists");
    let text = output(&listed);
    assert_eq!(
        serde_json::from_str::<Value>(&text).expect("json"),
        json!({"agents": [{"name": "Desk"}, {"name": "Research"}]})
    );
    assert!(!text.contains("secret-route"));

    let error = tool
        .run(
            call("call", json!({"agent": "Desk"})),
            CancellationToken::new(),
        )
        .await
        .expect_err("a field is refused");
    assert_eq!(error.code(), ToolErrorCode::InvalidInput);
    assert!(
        error
            .to_string()
            .contains("`agent` is not a field of agent_roster")
    );
    assert!(error.to_string().contains("A correct call is {}"));
}

//! `renoa.owner` through real agent sessions: an ask ends the turn and the
//! owner's next message answers it, an update leaves the turn going, and the
//! feed holds one item for each call.

use std::{fs, path::Path, sync::Arc};

use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock, ToolCall};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, AgentSession, FeedItem,
    FeedKind, HostFeed, LocalTurnOutcome, ModelProvider, Speaker, TurnObservation, Urgency,
    host::{HostInitialization, LocalHost},
    plugins::host::{HostPluginId, state},
};

/// `ask` calls `ask_owner` twice in one response; `tell` calls
/// `notify_owner` and answers with its result, prefixed `error: ` when it
/// failed. Any other prompt answers with the conversation the model received,
/// one `role: text` per message, each after a line holding `§`.
const MODEL: &str = r#"
import { createHash } from "node:crypto";
const spec = JSON.stringify({id:"fixture"});
if (process.env.RENOA_MODEL_ACTION === "catalog") {
  console.log(JSON.stringify({ok:true,response:{models:[{id:"fixture",name:"Fixture",reasoning_levels:["high"],context_window_tokens:500000,model_spec:{id:"fixture"}}]}}));
} else if (process.env.RENOA_MODEL_ACTION === "describe") {
  console.log(JSON.stringify({ok:true,response:{context_window_tokens:500000,max_output_tokens:32768,model_spec:spec,model_binding_id:createHash("sha256").update(spec).digest("hex"),reasoning_level:"high"}}));
} else {
  let input=""; for await (const part of process.stdin) input+=part;
  const request=JSON.parse(input);
  const complete=(content,stop_reason="stop")=>console.log(JSON.stringify({event:"completed",response:{content,stop_reason,usage:{input:10,output:1,cache_read:0,cache_write:0},metadata:{api:"fixture",provider:"xai",model:"fixture"}}}));
  const say=text=>complete([{type:"text",text}]);
  const index=request.messages.findLastIndex(message=>message.role==="user");
  const prompt=request.messages[index].content[0].text;
  const results=request.messages.slice(index+1).filter(message=>message.role==="tool").map(message=>message.result);
  const ask={question:"Book the 6pm table?",choices:["Yes","No"],urgency:"soon"};
  if (prompt==="ask") complete([
    {type:"tool_call",id:"first",name:"ask_owner",arguments:ask},
    {type:"tool_call",id:"second",name:"ask_owner",arguments:{...ask,question:"And a cake?"}},
  ],"tool_use");
  else if (prompt==="tell") {
    const last=results.at(-1);
    if (!last) complete([{type:"tool_call",id:"tell",name:"notify_owner",arguments:{message:"The build failed twice.",urgency:"soon"}}],"tool_use");
    else say((last.is_error?"error: ":"")+last.content[0].text);
  } else say(request.messages.map(message=>message.role+": "+(
    message.role==="tool" ? message.result.content[0].text
    : message.role==="user" ? message.content[0].text
    : message.content.map(part=>part.type==="text"?part.text:part.type==="tool_call"?part.name:"").join("|")
  )).join("\n§\n"));
}
"#;

const QUESTION: &str = "Book the 6pm table?\n- Yes\n- No";
const NOT_RUN: &str =
    "Not run: your turn ended when you asked the owner. Put everything you need into one question.";

struct Quiet;

impl AgentEventSink for Quiet {
    fn emit(&self, _: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

fn host(root: &Path) -> LocalHost {
    fs::write(root.join("model.mjs"), MODEL).expect("model");
    fs::write(root.join("auth.sqlite"), "").expect("auth boundary");
    LocalHost::assemble(HostInitialization {
        data_directory: root.join("data"),
        bridge: root.join("model.mjs"),
        providers: vec![ModelProvider::Xai],
        initial_provider: ModelProvider::Xai,
        initial_model: "fixture".to_owned(),
        initial_reasoning: None,
        credential_store: root.join("auth.sqlite"),
        mcp_adapter: None,
        mcp_registry_adapter: None,
        shared_plugin_registry: None,
        global_skill_source: None,
        oauth_relay: None,
        code_mode: None,
    })
    .expect("host")
}

async fn renoa(host: &LocalHost, root: &Path) -> (renoa_kernel::AgentId, Arc<AgentSession>) {
    let agent = host
        .create_agent(
            AgentCreator::System {
                component: "owner-fixture".to_owned(),
            },
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::from_preset(
                Uuid::new_v4(),
                AgentPresetId::new(crate::presets::APP_PRESET_ID).expect("preset"),
                "Renoa",
            )
            .with_instructions("Answer."),
            CancellationToken::new(),
        )
        .await
        .expect("agent")
        .id;
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace");
    let session = host
        .ensure_agent_session(agent, &workspace, Uuid::new_v4())
        .await
        .expect("session");
    (agent, session)
}

async fn turn(session: &AgentSession, speaker: Speaker, prompt: &str) -> LocalTurnOutcome {
    session
        .execute_turn_observed_with_cancellation(
            Uuid::new_v4(),
            vec![ContentBlock::text(prompt)],
            TurnObservation::now().expect("observation"),
            Arc::new(Quiet),
            CancellationToken::new(),
            speaker,
        )
        .await
        .unwrap_or_else(|error| panic!("{prompt} failed: {error}"))
}

async fn say(session: &AgentSession, speaker: Speaker, prompt: &str) -> String {
    match turn(session, speaker, prompt).await {
        LocalTurnOutcome::Completed { output, .. } => output,
        other => panic!("{prompt} did not complete: {other:?}"),
    }
}

async fn items(root: &Path) -> Vec<FeedItem> {
    HostFeed::open(&root.join("data"))
        .expect("feed")
        .list(None)
        .await
        .expect("list")
        .items
}

#[tokio::test]
async fn an_ask_ends_the_turn_and_the_owners_next_message_answers_it() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let host = host(root);
    let (_, session) = renoa(&host, root).await;
    let owner = Speaker::Principal(Uuid::new_v4());

    assert_eq!(
        turn(&session, owner, "ask").await,
        LocalTurnOutcome::WaitingForInput {
            output: QUESTION.to_owned()
        }
    );
    let [asked] = items(root).await.try_into().expect("one item");
    assert_eq!(
        (asked.kind, asked.urgency, asked.done, asked.body.as_str()),
        (FeedKind::Ask, Urgency::Soon, false, "Book the 6pm table?")
    );
    assert_eq!(asked.choices, Some(vec!["Yes".to_owned(), "No".to_owned()]));
    assert_eq!(asked.subject_id, session.id().to_string());

    say(&session, Speaker::Guest, "Maybe later").await;
    assert!(!items(root).await[0].done, "a guest's message is no answer");

    let seen = say(&session, owner, "Yes").await;
    let tail: Vec<&str> = seen.rsplit("\n§\n").take(6).collect();
    assert_eq!(
        tail,
        [
            "user: Yes",
            "user: Maybe later",
            &format!("assistant: {QUESTION}"),
            &format!("tool: {NOT_RUN}"),
            "tool: The owner has your question. Their answer comes as a new message.",
            "assistant: ask_owner|ask_owner",
        ],
        "{seen}"
    );
    assert!(items(root).await[0].done, "the owner's message answers it");
}

#[tokio::test]
async fn an_update_reaches_the_feed_and_the_turn_goes_on() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let host = host(root);
    let (_, session) = renoa(&host, root).await;

    assert_eq!(
        say(&session, Speaker::Principal(Uuid::new_v4()), "tell").await,
        "The owner will see this in their feed."
    );
    let [told] = items(root).await.try_into().expect("one item");
    assert_eq!(
        (told.kind, told.urgency, told.choices, told.body.as_str()),
        (
            FeedKind::Update,
            Urgency::Soon,
            None,
            "The build failed twice."
        )
    );
    assert_eq!(told.subject_id, session.id().to_string());
}

#[tokio::test]
async fn the_owner_tools_refuse_while_turned_off_and_for_a_guest() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let host = host(root);
    let (agent, session) = renoa(&host, root).await;

    let guest = say(&session, Speaker::Guest, "tell").await;
    assert!(
        guest.starts_with("error: ") && guest.contains("guest"),
        "{guest}"
    );
    state::change(
        &host.config.database,
        agent,
        HostPluginId::Owner,
        false,
        "owner-off",
    )
    .expect("off");
    assert_eq!(
        say(&session, Speaker::Principal(Uuid::new_v4()), "tell").await,
        "error: notify_owner is turned off for this agent, so nothing was sent. Say it in your reply instead."
    );
    assert!(items(root).await.is_empty(), "nothing was sent");
}

/// A replayed call rewrites its item, and a failed write is an error the
/// model can act on, with nothing written.
#[tokio::test]
async fn a_replay_keeps_one_item_and_an_unsaved_ask_lets_the_turn_go_on() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let host = host(root);
    let (agent, session) = renoa(&host, root).await;
    let bindings = |database: &Path| {
        crate::plugins::host::owner::bindings(crate::plugins::host::owner::OwnerTurn {
            database: database.to_path_buf(),
            agent,
            session: renoa_kernel::SessionId::from_uuid(session.id()),
            command: Some(renoa_kernel::CommandId::new()),
        })
    };
    let ask = |binding: &renoa_agent_loop::AgentToolBinding| {
        let tool = binding.tool();
        async move {
            renoa_agent::invoke_tool(
                Some(tool.as_ref()),
                ToolCall {
                    id: "call".to_owned(),
                    name: "ask_owner".to_owned(),
                    arguments: json!({"question": "Book it?", "urgency": "whenever"}),
                    namespace: None,
                    thought_signature: None,
                },
                CancellationToken::new(),
                None,
            )
            .await
            .expect("definite result")
        }
    };
    let [asking, _] = bindings(&host.config.database);
    let first = ask(&asking).await;
    let second = ask(&asking).await;
    assert_eq!(first, second);
    assert!(!first.is_error);
    assert_eq!(items(root).await.len(), 1, "a replay writes no second item");

    let [unsaved, _] = bindings(&root.join("missing.sqlite3"));
    let failed = ask(&unsaved).await;
    assert!(failed.is_error && failed.closing.is_none());
    assert_eq!(
        failed.content,
        vec![ContentBlock::text(
            "The question could not be saved, so nothing was sent and your turn continues. Try once more; if it fails again, tell the owner in your reply."
        )]
    );
}

/// Schema 48 admits `renoa.owner` and gives stored items an urgency.
#[tokio::test]
async fn schema_48_admits_the_owner_plugin_into_a_schema_47_catalog() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let host = host(root);
    let (agent, session) = renoa(&host, root).await;
    let database = host.config.database.clone();
    let connection = rusqlite::Connection::open(&database).expect("catalog");
    crate::host::catalog::restore_schema_47(&connection);
    connection
        .execute_batch(
            "UPDATE host_metadata SET schema_version = 47 WHERE singleton = 1;
             PRAGMA user_version = 47;",
        )
        .expect("store schema 47");
    let refused = connection.execute(
        "INSERT INTO host_agent_builtin_plugins VALUES (?1, 'renoa.owner', 0)",
        [agent.to_string()],
    );
    assert!(refused.is_err(), "schema 47 knows no renoa.owner");
    connection
        .execute(
            "INSERT INTO host_feed_items(id, kind, agent_id, title, body, action, subject_id, created_at_ms)
             VALUES (?1, 'ability', ?2, 'Turned on Git', '', 'turn_off', 'renoa.git', 1)",
            [Uuid::new_v4().to_string(), agent.to_string()],
        )
        .expect("a schema 47 item");
    drop(connection);
    crate::host::catalog::initialize(&database).expect("upgrade schema 47");
    crate::host::catalog::initialize(&database).expect("reopen the upgraded catalog");

    state::change(&database, agent, HostPluginId::Owner, false, "owner-off").expect("off");
    state::change(&database, agent, HostPluginId::Owner, true, "owner-on").expect("on");
    say(&session, Speaker::Principal(Uuid::new_v4()), "tell").await;
    let [told, turned_on, stored] = items(root).await.try_into().expect("three items");
    assert_eq!(turned_on.kind, FeedKind::Ability);
    assert_eq!(
        (told.urgency, stored.urgency, stored.choices),
        (Urgency::Soon, Urgency::Whenever, None),
        "an upgrade leaves a stored item quiet"
    );
}

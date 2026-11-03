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
/// `notify_owner` and `today` calls `set_today`, each answering with its
/// result, prefixed `error: ` when it failed. `Yes` answers with the conversation the model received, one
/// `role: text` per message, each after a line holding `§`; any other prompt
/// answers `Noted.`.
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
  } else if (prompt==="today") {
    const last=results.at(-1);
    if (!last) complete([{type:"tool_call",id:"today",name:"set_today",arguments:{sentence:"Your 3pm call moved to 4."}}],"tool_use");
    else say((last.is_error?"error: ":"")+last.content[0].text);
  } else if (prompt!=="Yes") say("Noted.");
  else say(request.messages.map(message=>message.role+": "+(
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
    turn_as(session, Uuid::new_v4(), speaker, prompt).await
}

/// A turn under `command`, as a scheduled run's turn has the run's id.
async fn turn_as(
    session: &AgentSession,
    command: Uuid,
    speaker: Speaker,
    prompt: &str,
) -> LocalTurnOutcome {
    session
        .execute_turn_observed_with_cancellation(
            command,
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
    say(&session, Speaker::Unidentified, "From another agent").await;
    assert!(
        !items(root).await[0].done,
        "a guest's or another agent's message is no answer"
    );

    let seen = say(&session, owner, "Yes").await;
    let messages: Vec<&str> = seen.split("\n§\n").collect();
    let call = messages
        .iter()
        .position(|message| *message == "assistant: ask_owner|ask_owner")
        .unwrap_or_else(|| panic!("the call is in history: {seen}"));
    assert_eq!(
        messages[call..call + 4],
        [
            "assistant: ask_owner|ask_owner",
            "tool: The owner has your question. Their answer comes as a new message.",
            &format!("tool: {NOT_RUN}"),
            &format!("assistant: {QUESTION}"),
        ],
        "{seen}"
    );
    assert_eq!(messages.last(), Some(&"user: Yes"));
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
    let [asking, ..] = bindings(&host.config.database);
    let first = ask(&asking).await;
    let second = ask(&asking).await;
    assert_eq!(first, second);
    assert!(!first.is_error);
    assert_eq!(items(root).await.len(), 1, "a replay writes no second item");

    let [unsaved, ..] = bindings(&root.join("missing.sqlite3"));
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

/// Deleting a conversation removes the asks and updates made in it: nothing
/// can answer them, and an open ask would wait forever.
#[tokio::test]
async fn deleting_a_conversation_removes_its_asks() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let host = host(root);
    let (agent, session) = renoa(&host, root).await;
    let owner = Speaker::Principal(Uuid::new_v4());
    turn(&session, owner, "ask").await;
    say(&session, owner, "tell").await;
    assert_eq!(items(root).await.len(), 2);

    let id = session.id();
    drop(session);
    host.delete_session(agent, id).await.expect("deleted");
    assert!(items(root).await.is_empty(), "{:?}", items(root).await);
}

const HOUR_MS: i64 = 3_600_000;
const NOT_A_HEARTBEAT: &str = "set_today only works during your heartbeat, and this turn is not one. To tell the owner something now, use notify_owner.";

fn now_ms() -> i64 {
    TurnObservation::now().expect("now").unix_milliseconds()
}

/// The heartbeat trigger of `agent` as stored: its schedule and whether it
/// is on, or `None` while it has none.
fn heartbeat(host: &LocalHost, agent: renoa_kernel::AgentId) -> Option<(serde_json::Value, bool)> {
    let id = crate::plugins::host::triggers::automation_id(HostPluginId::Owner, agent);
    let db = rusqlite::Connection::open(&host.config.database).expect("catalog");
    rusqlite::OptionalExtension::optional(db.query_row(
        "SELECT schedule_json, enabled FROM host_automations WHERE id = ?1
         AND id IN (SELECT automation_id FROM host_plugin_triggers WHERE plugin_id = 'renoa.owner')",
        [id.to_string()],
        |row| {
            Ok((
                serde_json::from_str(&row.get::<_, String>(0)?).expect("schedule"),
                row.get(1)?,
            ))
        },
    ))
    .expect("trigger")
}

fn every(hours: u8) -> serde_json::Value {
    let zone = jiff::tz::TimeZone::try_system().unwrap_or(jiff::tz::TimeZone::UTC);
    json!({
        "kind": "cron",
        "expression": format!("0 8-22/{hours} * * *"),
        "timezone": zone.iana_name().unwrap_or("UTC"),
    })
}

/// No agent checks in until its setting says how often, in the hours of the
/// waking day, and none while `renoa.owner` is off for it. The scheduler
/// converges each change before it next admits a run.
#[tokio::test]
async fn a_heartbeat_follows_its_agents_setting_and_switch() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let host = host(root);
    let (main, _) = renoa(&host, root).await;
    let other = host
        .create_agent(
            AgentCreator::System {
                component: "owner-fixture".to_owned(),
            },
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::from_preset(
                Uuid::new_v4(),
                AgentPresetId::new(crate::presets::APP_PRESET_ID).expect("preset"),
                "Scout",
            ),
            CancellationToken::new(),
        )
        .await
        .expect("agent")
        .id;
    let scheduler = host.automation_scheduler().expect("scheduler");
    let converge = || async { scheduler.next_run(now_ms(), false).await.expect("tick") };

    converge().await;
    assert_eq!(host.heartbeat_hours(main).await.expect("read"), Some(0));
    assert_eq!(heartbeat(&host, main), None, "off until set");

    let set = |agent, hours, operation| host.set_heartbeat_hours(agent, hours, operation);
    assert_eq!(set(main, 3, Uuid::new_v4()).await.expect("set"), Some(3));
    converge().await;
    assert_eq!(heartbeat(&host, main), Some((every(3), true)));
    assert_eq!(set(other, 6, Uuid::new_v4()).await.expect("set"), Some(6));
    let off = Uuid::new_v4();
    assert_eq!(set(main, 0, off).await.expect("off"), Some(0));
    assert_eq!(set(main, 0, off).await.expect("replay"), Some(0));
    assert!(matches!(
        set(main, 2, off).await,
        Err(crate::LocalHostError::Plugin(crate::PluginError::Conflict(
            _
        )))
    ));
    assert!(matches!(
        set(main, 25, Uuid::new_v4()).await,
        Err(crate::LocalHostError::Plugin(crate::PluginError::Invalid(
            _
        )))
    ));
    converge().await;
    assert_eq!(heartbeat(&host, main), Some((every(3), false)), "kept off");
    assert_eq!(heartbeat(&host, other), Some((every(6), true)));

    set(main, 2, Uuid::new_v4()).await.expect("on");
    state::change(
        &host.config.database,
        other,
        HostPluginId::Owner,
        false,
        "off",
    )
    .expect("switch off");
    converge().await;
    assert_eq!(heartbeat(&host, main), Some((every(2), true)));
    assert_eq!(heartbeat(&host, other), Some((every(6), false)));
    assert_eq!(
        host.heartbeat_hours(renoa_kernel::AgentId::new())
            .await
            .expect("read"),
        None
    );
}

/// A heartbeat's run goes to its agent, quiet, on a task of its own, with
/// the standing task; `set_today` refuses in any other turn and sets the
/// Today line in that one.
#[tokio::test]
async fn set_today_works_only_in_a_run_of_the_agents_heartbeat() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let host = host(root);
    let (agent, session) = renoa(&host, root).await;
    let owner = Speaker::Principal(Uuid::new_v4());
    let today = || async {
        HostFeed::open(&root.join("data"))
            .expect("feed")
            .list(None)
            .await
            .expect("list")
            .today
    };

    assert_eq!(
        say(&session, owner, "today").await,
        format!("error: {NOT_A_HEARTBEAT}")
    );
    assert_eq!(today().await, "All quiet.");

    host.set_heartbeat_hours(agent, 1, Uuid::new_v4())
        .await
        .expect("turn the heartbeat on");
    let scheduler = host.automation_scheduler().expect("scheduler");
    let now = now_ms();
    scheduler.next_run(now, false).await.expect("converge");
    let id = crate::plugins::host::triggers::automation_id(HostPluginId::Owner, agent);
    let due_ms: i64 = rusqlite::Connection::open(&host.config.database)
        .expect("catalog")
        .query_row(
            "SELECT next_due_ms FROM host_automations WHERE id = ?1",
            [id.to_string()],
            |row| row.get(0),
        )
        .expect("the heartbeat is scheduled");
    assert!(due_ms > now && due_ms <= now + 11 * HOUR_MS, "{due_ms}");
    let due = scheduler
        .next_run(due_ms, false)
        .await
        .expect("tick")
        .expect("the heartbeat is due");
    assert_eq!(
        (
            due.run.automation_id,
            due.plugin,
            due.quiet,
            due.origin_session_id,
            due.origin_required
        ),
        (id, None, true, None, false)
    );
    assert!(
        due.run.submission.contains("This is your heartbeat")
            && due.run.submission.contains("set_today"),
        "{}",
        due.run.submission
    );

    let LocalTurnOutcome::Completed { output, .. } =
        turn_as(&session, due.run.id, owner, "today").await
    else {
        panic!("the heartbeat's turn completes");
    };
    assert_eq!(output, "The owner's home shows this line now.");
    assert_eq!(today().await, "Your 3pm call moved to 4.");
}

/// Schema 50 lets `renoa.owner` keep settings and triggers, and keeps every
/// stored setting, trigger and the reminder that refers to its trigger.
#[tokio::test]
async fn schema_50_lets_the_owner_plugin_keep_settings_and_triggers() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let host = host(root);
    let (agent, _) = renoa(&host, root).await;
    host.set_heartbeat_hours(agent, 3, Uuid::new_v4())
        .await
        .expect("turn the heartbeat on");
    host.automation_scheduler()
        .expect("scheduler")
        .next_run(now_ms(), false)
        .await
        .expect("converge the nightly trigger and the heartbeat");
    assert!(heartbeat(&host, agent).is_some());
    let database = host.config.database.clone();
    let connection = rusqlite::Connection::open(&database).expect("catalog");
    let nightly = crate::plugins::host::triggers::automation_id(HostPluginId::Memory, agent);
    connection
        .execute(
            "INSERT INTO host_memory_reminders(automation_id, principal_id, item_id, due, attempt)
             VALUES (?1, ?2, 'item', '2026-11-03T09:00', 0)",
            [nightly.to_string(), Uuid::new_v4().to_string()],
        )
        .expect("a reminder that refers to a trigger");
    connection
        .execute(
            "INSERT INTO host_agent_plugin_settings VALUES (?1, 'renoa.time', '{\"timezone\":\"Asia/Seoul\"}')",
            [agent.to_string()],
        )
        .expect("a time setting");
    crate::host::catalog::restore_schema_49(&connection);
    connection
        .execute_batch(
            "UPDATE host_metadata SET schema_version = 49 WHERE singleton = 1;
             PRAGMA user_version = 49;",
        )
        .expect("store schema 49");
    let refused = connection.execute(
        "INSERT INTO host_agent_plugin_settings VALUES (?1, 'renoa.owner', '{}')",
        [agent.to_string()],
    );
    assert!(refused.is_err(), "schema 49 keeps no renoa.owner settings");
    assert_eq!(heartbeat(&host, agent), None);
    drop(connection);

    crate::host::catalog::initialize(&database).expect("upgrade schema 49");
    crate::host::catalog::initialize(&database).expect("reopen the upgraded catalog");
    let connection = rusqlite::Connection::open(&database).expect("catalog");
    let kept: (String, String, i64) = connection
        .query_row(
            "SELECT (SELECT settings_json FROM host_agent_plugin_settings WHERE plugin_id = 'renoa.time'),
                    (SELECT plugin_id FROM host_plugin_triggers WHERE automation_id = ?1),
                    (SELECT count(*) FROM host_memory_reminders WHERE automation_id = ?1)",
            [nightly.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("kept rows");
    assert_eq!(
        kept,
        (
            "{\"timezone\":\"Asia/Seoul\"}".to_owned(),
            "renoa.memory".to_owned(),
            1
        )
    );
    drop(connection);
    assert_eq!(
        host.set_heartbeat_hours(agent, 4, Uuid::new_v4())
            .await
            .expect("set"),
        Some(4)
    );
    host.automation_scheduler()
        .expect("scheduler")
        .next_run(now_ms(), false)
        .await
        .expect("converge");
    assert!(heartbeat(&host, agent).is_some_and(|(_, on)| on));
}

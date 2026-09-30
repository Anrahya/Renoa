//! `renoa.time` through a real agent session: the context each message is
//! admitted with, its settings, and which agents have it.

use std::{fs, path::Path, sync::Arc};

use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, AgentSession,
    LocalTurnOutcome, ModelProvider, TurnObservation,
    host::{HostInitialization, LocalHost},
    plugins::{
        api::{PluginInventoryItem, PluginInvocation, PluginOutcome, PluginRequest},
        host::{HostPluginId, settings, state},
    },
};

/// Answers "set zone <name>" by calling `configure_plugin`, and anything else
/// with the context blocks of every user message it was sent, in order.
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
  const users=request.messages.filter(message=>message.role==="user");
  const prompt=users.at(-1).content[0].text;
  const index=request.messages.findLastIndex(message=>message.role==="user");
  const results=request.messages.slice(index+1).filter(message=>message.role==="tool");
  if (prompt.startsWith("set zone ")) {
    const zone=prompt.slice("set zone ".length);
    if (!results.length) complete([{type:"tool_call",id:"configure",name:"plugin_manage",arguments:{action:"configure_plugin",plugin_id:"renoa.time",settings:zone==="default"?{}:{timezone:zone}}}],"tool_use");
    else complete([{type:"text",text:results[0].result.is_error?"refused":"configured"}]);
  } else {
    complete([{type:"text",text:JSON.stringify(users.map(user=>user.content.slice(1).map(block=>block.text).join("|")))}]);
  }
}
"#;

/// 2026-08-31T18:04:05Z.
const T0: i64 = 1_788_199_445_000;
const HOUR: i64 = 3_600_000;

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

async fn agent(host: &LocalHost, preset: &str) -> renoa_kernel::AgentId {
    host.create_agent(
        AgentCreator::System {
            component: "time-fixture".to_owned(),
        },
        AgentCreationOrigin::Provisioning,
        AgentCreateRequest::from_preset(
            Uuid::new_v4(),
            AgentPresetId::new(preset).expect("preset"),
            "Timed",
        )
        .with_instructions("Answer."),
        CancellationToken::new(),
    )
    .await
    .expect("agent")
    .id
}

async fn say(session: &AgentSession, prompt: &str, at: i64) -> String {
    let observation = TurnObservation::from_unix_milliseconds(at).expect("observation");
    say_observed(session, prompt, observation).await
}

async fn say_observed(
    session: &AgentSession,
    prompt: &str,
    observation: TurnObservation,
) -> String {
    say_as(session, Uuid::new_v4(), prompt, observation).await
}

/// One prompt under a caller-chosen command identity, as a retry sends it.
async fn say_as(
    session: &AgentSession,
    command: Uuid,
    prompt: &str,
    observation: TurnObservation,
) -> String {
    match session
        .execute_turn_observed(
            command,
            vec![ContentBlock::text(prompt)],
            observation,
            Arc::new(Quiet),
        )
        .await
        .unwrap_or_else(|error| panic!("{prompt} failed: {error}"))
    {
        LocalTurnOutcome::Completed { output, .. } => output,
        other => panic!("{prompt} did not complete: {other:?}"),
    }
}

/// The context each user message carried, as the model received it.
async fn contexts(session: &AgentSession, at: i64) -> Vec<String> {
    serde_json::from_str(&say(session, "show", at).await).expect("context list")
}

fn time(current: &str, elapsed: Option<&str>) -> String {
    let elapsed = elapsed.map_or_else(String::new, |e| {
        format!("\nelapsed_since_previous_user_message: {e}")
    });
    format!(
        "<turn_context>\n<context source=\"plugin:renoa.time\">\ncurrent_time: {current}{elapsed}\n</context>\n</turn_context>"
    )
}

fn receipts(host: &LocalHost) -> i64 {
    rusqlite::Connection::open(&host.config.database)
        .expect("catalog")
        .query_row(
            "SELECT count(*) FROM host_builtin_plugin_operations",
            [],
            |row| row.get(0),
        )
        .expect("receipt count")
}

#[tokio::test]
async fn each_message_keeps_the_time_it_was_admitted_with_in_the_agent_zone() {
    let directory = tempfile::tempdir().expect("directory");
    let host = host(directory.path());
    let agent = agent(&host, crate::presets::GENERAL_PRESET_ID).await;
    let workspace = directory.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace");
    let session = host
        .ensure_agent_session(agent, &workspace, Uuid::new_v4())
        .await
        .expect("session");

    assert_eq!(
        say(&session, "set zone Asia/Kolkata", T0).await,
        "configured"
    );
    let first = contexts(&session, T0 + HOUR).await;
    assert_eq!(
        first[1],
        time("2026-09-01T00:34:05+05:30[Asia/Kolkata]", Some("1h"))
    );

    assert_eq!(
        say(&session, "set zone Asia/Seoul", T0 + 2 * HOUR).await,
        "configured"
    );
    let before = receipts(&host);
    assert_eq!(
        say(&session, "set zone Mars/Olympus", T0 + 3 * HOUR).await,
        "refused"
    );
    assert_eq!(receipts(&host), before, "a refused zone leaves no receipt");
    let later = contexts(&session, T0 + 4 * HOUR).await;
    assert_eq!(later[..2], first[..], "earlier messages keep their bytes");
    assert_eq!(
        later[4],
        time("2026-09-01T07:04:05+09:00[Asia/Seoul]", Some("1h"))
    );
    assert_eq!(
        settings::read(&host.config.database, agent, HostPluginId::Time).expect("settings"),
        json!({"timezone": "Asia/Seoul"})
    );

    let PluginOutcome::Listed(page) = host
        .config
        .plugins
        .invoke(
            &agent,
            &workspace,
            PluginRequest::List {
                cursor: None,
                limit: 200,
            },
            PluginInvocation {
                operation_id: "list",
                updates: None,
                cancellation: CancellationToken::new(),
            },
        )
        .await
        .expect("inventory")
    else {
        panic!("list returns an inventory page");
    };
    let listed = page
        .items()
        .iter()
        .find_map(|item| match item {
            PluginInventoryItem::HostPlugin {
                activation,
                settings,
            } if activation.plugin_id == "renoa.time" => Some((activation.enabled, settings)),
            _ => None,
        })
        .expect("renoa.time is listed");
    assert_eq!(listed, (true, &Some(json!({"timezone": "Asia/Seoul"}))));

    state::change(
        &host.config.database,
        agent,
        HostPluginId::Time,
        false,
        "turn-time-off",
    )
    .expect("turn renoa.time off");
    let off = contexts(&session, T0 + 5 * HOUR).await;
    assert_eq!(off[..5], later[..], "turning it off rewrites nothing");
    assert_eq!(off[5], "", "a message admitted while it is off has no time");
}

#[tokio::test]
async fn the_surface_says_where_a_message_was_written_ahead_of_every_plugin() {
    let directory = tempfile::tempdir().expect("directory");
    let host = host(directory.path());
    let general = agent(&host, crate::presets::GENERAL_PRESET_ID).await;
    let untimed = agent(&host, crate::presets::ALPHA_PRESET_ID).await;
    let workspace = directory.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace");
    let session = |agent| host.ensure_agent_session(agent, &workspace, Uuid::new_v4());
    let general = session(general).await.expect("general session");
    let untimed = session(untimed).await.expect("untimed session");
    let placed = |at: i64, place: &str| {
        TurnObservation::from_unix_milliseconds(at)
            .expect("observation")
            .with_surface_context(place)
    };
    let shown =
        |output: String| -> Vec<String> { serde_json::from_str(&output).expect("context list") };

    assert_eq!(say(&general, "set zone UTC", T0).await, "configured");
    let desk = "Discord server 10\nchannel #desk & <tools> (202)";
    let first = shown(say_observed(&general, "show", placed(T0 + HOUR, desk)).await);
    assert_eq!(
        first[1],
        "<turn_context>\n<context source=\"surface\">\nDiscord server 10\nchannel #desk &amp; &lt;tools&gt; (202)\n</context>\n<context source=\"plugin:renoa.time\">\ncurrent_time: 2026-08-31T19:04:05+00:00[UTC]\nelapsed_since_previous_user_message: 1h\n</context>\n</turn_context>"
    );
    let unfit = shown(say_observed(&general, "show", placed(T0 + 2 * HOUR, "bell\u{7}")).await);
    assert_eq!(unfit[..2], first[..], "earlier messages keep their bytes");
    assert_eq!(
        unfit[2],
        time("2026-08-31T20:04:05+00:00[UTC]", Some("1h")),
        "a surface context that does not fit is left out and the plugins stay"
    );

    assert_eq!(
        shown(
            say_observed(
                &untimed,
                "show",
                placed(T0, "Discord direct message (channel 404)")
            )
            .await
        ),
        [
            "<turn_context>\n<context source=\"surface\">\nDiscord direct message (channel 404)\n</context>\n</turn_context>"
        ],
        "the surface entry does not depend on any plugin"
    );

    let command = Uuid::new_v4();
    let first = say_as(&untimed, command, "show", placed(T0, "channel #desk (202)")).await;
    let retried = say_as(
        &untimed,
        command,
        "show",
        placed(T0, "channel #other (303)"),
    )
    .await;
    assert_eq!(retried, first, "a retry of a finished command replays it");
    let history = shown(say_observed(&untimed, "show", placed(T0, "channel #desk (202)")).await);
    assert_eq!(
        history[1],
        "<turn_context>\n<context source=\"surface\">\nchannel #desk (202)\n</context>\n</turn_context>",
        "the replayed command keeps the context it was admitted with"
    );
}

#[tokio::test]
async fn the_coding_preset_starts_without_time_and_others_start_with_it() {
    let directory = tempfile::tempdir().expect("directory");
    let host = host(directory.path());
    let coding = agent(&host, crate::presets::ALPHA_PRESET_ID).await;
    let general = agent(&host, crate::presets::GENERAL_PRESET_ID).await;
    let database = &host.config.database;
    assert!(!state::enabled(database, coding, HostPluginId::Time).expect("coding"));
    assert!(state::enabled(database, general, HostPluginId::Time).expect("general"));

    let workspace = directory.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace");
    let session = host
        .ensure_agent_session(coding, &workspace, Uuid::new_v4())
        .await
        .expect("session");
    assert_eq!(contexts(&session, T0).await, [""]);
}

#[tokio::test]
async fn schema_41_moves_stored_turn_timing_onto_the_time_plugin() {
    let directory = tempfile::tempdir().expect("directory");
    let host = host(directory.path());
    let creator = AgentCreator::System {
        component: "time-fixture".to_owned(),
    };
    let request = |preset: &str| {
        AgentCreateRequest::from_preset(
            Uuid::new_v4(),
            AgentPresetId::new(preset).expect("preset"),
            "Migrated",
        )
    };
    let untimed = request(crate::presets::ALPHA_PRESET_ID);
    // An explicit behavior is part of the stored request, and the retry below
    // compares that request byte for byte.
    let mut timed = request(crate::presets::GENERAL_PRESET_ID)
        .with_instructions("Line one.\nZone: Asia/Kolkata — “quoted” ✓");
    timed.behavior = Some(crate::AgentBehavior {
        workspace_instructions: crate::WorkspaceInstructions::Off,
        automatic_compaction: None,
    });
    let mut ids = Vec::new();
    for request in [untimed.clone(), timed.clone()] {
        ids.push(
            host.create_agent(
                creator.clone(),
                AgentCreationOrigin::Provisioning,
                request,
                CancellationToken::new(),
            )
            .await
            .expect("agent")
            .id,
        );
    }
    let database = host.config.database.clone();
    let connection = rusqlite::Connection::open(&database).expect("catalog");
    crate::host::catalog::restore_schema_40(&connection);
    connection
        .execute_batch(&format!(
            "UPDATE host_agents SET operational_json = json_set(operational_json,
                '$.behavior.turn_timing', CASE agent_id WHEN '{0}' THEN 'off' ELSE 'host_clock' END);
             UPDATE host_agent_creations SET result_json = json_set(result_json,
                '$.operational.behavior.turn_timing', CASE agent_id WHEN '{0}' THEN 'off' ELSE 'host_clock' END);
             UPDATE host_agent_creations SET request_json = json_set(request_json,
                '$.behavior.turn_timing', 'host_clock')
                WHERE json_type(request_json, '$.behavior') = 'object';
             INSERT INTO host_agent_renames(operation_id, agent_id, actor_agent_id,
                request_json, result_json)
                SELECT 'rename', agent_id, agent_id, '{{}}', result_json
                FROM host_agent_creations WHERE agent_id = '{0}';
             UPDATE host_metadata SET schema_version = 40 WHERE singleton = 1;
             PRAGMA user_version = 40;",
            ids[0]
        ))
        .expect("store the schema 40 behavior");
    drop(connection);

    crate::host::catalog::initialize(&database).expect("upgrade schema 40");
    crate::host::catalog::initialize(&database).expect("reopen the upgraded catalog");

    assert!(!state::enabled(&database, ids[0], HostPluginId::Time).expect("untimed"));
    assert!(state::enabled(&database, ids[1], HostPluginId::Time).expect("timed"));
    let leftover: i64 = rusqlite::Connection::open(&database)
        .expect("catalog")
        .query_row(
            "SELECT (SELECT count(*) FROM host_agents WHERE operational_json LIKE '%turn_timing%')
                  + (SELECT count(*) FROM host_agent_creations WHERE result_json LIKE '%turn_timing%')
                  + (SELECT count(*) FROM host_agent_creations WHERE request_json LIKE '%turn_timing%')
                  + (SELECT count(*) FROM host_agent_renames WHERE result_json LIKE '%turn_timing%')",
            [],
            |row| row.get(0),
        )
        .expect("leftover count");
    assert_eq!(leftover, 0);
    for (request, id) in [untimed, timed].into_iter().zip(ids) {
        let replayed = host
            .create_agent(
                creator.clone(),
                AgentCreationOrigin::Provisioning,
                request,
                CancellationToken::new(),
            )
            .await
            .expect("a creation retried after the upgrade replays its receipt");
        assert_eq!(replayed.id, id);
    }
}

#[tokio::test]
async fn every_compiled_plugin_fits_its_tables_and_configure_replays_its_receipt() {
    let directory = tempfile::tempdir().expect("directory");
    let host = host(directory.path());
    let agent = agent(&host, crate::presets::GENERAL_PRESET_ID).await;
    let database = &host.config.database;
    let connection = rusqlite::Connection::open(database).expect("catalog");
    for plugin in HostPluginId::ALL {
        connection
            .execute(
                "INSERT OR REPLACE INTO host_agent_builtin_plugins(agent_id, plugin_id, enabled)
                 VALUES (?1, ?2, 1)",
                rusqlite::params![agent.to_string(), plugin.id()],
            )
            .unwrap_or_else(|error| panic!("{} is missing from the CHECK: {error}", plugin.id()));
        let stored = connection.execute(
            "INSERT OR REPLACE INTO host_agent_plugin_settings(agent_id, plugin_id, settings_json)
             VALUES (?1, ?2, '{}')",
            rusqlite::params![agent.to_string(), plugin.id()],
        );
        assert_eq!(
            stored.is_ok(),
            settings::configurable(plugin),
            "the settings CHECK and the plugin's validator disagree on {}",
            plugin.id()
        );
    }
    connection
        .execute_batch(
            "DELETE FROM host_agent_plugin_settings; DELETE FROM host_agent_builtin_plugins;",
        )
        .expect("clear the probe rows");
    drop(connection);

    let seoul = json!({"timezone": "Asia/Seoul"});
    let first = settings::configure(database, agent, HostPluginId::Time, seoul.clone(), "op")
        .expect("configure");
    let before = receipts(&host);
    assert_eq!(
        settings::configure(database, agent, HostPluginId::Time, seoul, "op").expect("replay"),
        first
    );
    assert_eq!(receipts(&host), before, "a replay writes nothing");
    assert!(matches!(
        settings::configure(database, agent, HostPluginId::Time, json!({}), "op"),
        Err(crate::plugins::PluginError::Conflict(_))
    ));
    state::change(database, agent, HostPluginId::Time, false, "toggle").expect("turn off");
    assert!(matches!(
        settings::configure(database, agent, HostPluginId::Time, json!({}), "toggle"),
        Err(crate::plugins::PluginError::Conflict(_))
    ));
    assert!(matches!(
        state::change(database, agent, HostPluginId::Time, true, "op"),
        Err(crate::plugins::PluginError::Conflict(_))
    ));
    assert!(
        settings::configure(database, agent, HostPluginId::Git, json!({}), "git").is_err(),
        "a plugin without settings refuses them"
    );
}

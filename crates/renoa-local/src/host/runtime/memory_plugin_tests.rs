//! `renoa.memory` through real agent sessions: whose notebook a turn reads and
//! writes, and the `MIND.md` block its system prompt carries.

use std::{fs, path::Path, sync::Arc};

use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, AgentSession,
    LocalHostError, LocalTurnOutcome, ModelProvider, Speaker, TurnObservation,
    host::{HostInitialization, LocalHost},
    plugins::host::{HostPluginId, memory::fixtures, settings, state},
};

/// `note: <text>`, `recall: <pattern>` and `zoom: <block>` call that memory
/// tool through plugin discovery and answer with its result, prefixed `error: `
/// when it failed. `prompt` answers with the whole system prompt, `context`
/// with the user message as the model received it, and `show`
/// with its `<session_context>` element or `none`; `show` hangs after writing
/// `started` beside this file while a `hang` file is there.
const MODEL: &str = r#"
import { createHash } from "node:crypto";
import { existsSync, writeFileSync } from "node:fs";
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
  const call=(id,name,args)=>complete([{type:"tool_call",id,name,arguments:args}],"tool_use");
  const index=request.messages.findLastIndex(message=>message.role==="user");
  const prompt=request.messages[index].content[0].text;
  const results=request.messages.slice(index+1).filter(message=>message.role==="tool").map(message=>message.result);
  const verb=prompt.split(": ")[0];
  const tools={note:["memory_note","text"],recall:["memory_recall","pattern"],zoom:["memory_zoom","block"]};
  if (prompt==="prompt") say(request.system_prompt);
  else if (prompt==="context") say(JSON.stringify(request.messages[index].content));
  else if (prompt==="show") {
    if (existsSync(new URL("./hang", import.meta.url))) {
      writeFileSync(new URL("./started", import.meta.url), "");
      setInterval(()=>{}, 1000);
    } else say(request.system_prompt.match(/<session_context>[\s\S]*<\/session_context>/)?.[0] ?? "none");
  } else if (tools[verb]) {
    const [name,field]=tools[verb];
    const last=results.at(-1);
    if (!last) call("search","plugin_search",{query:name});
    else if (last.is_error) say("error: "+last.content[0].text);
    else if (last.name==="plugin_search") {
      const value=JSON.parse(last.content[0].text);
      const match=value.tool_matches?.find(tool=>tool.name===name) ?? value;
      if (!match.input_schema) call("schema","plugin_search",{reference:match.reference});
      else call("run","tool_execute",{reference:match.reference,arguments:{[field]:prompt.slice(verb.length+2)}});
    } else say(last.content[0].text);
  } else say("unknown prompt");
}
"#;

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

async fn agent(host: &LocalHost, preset: &str, name: &str) -> renoa_kernel::AgentId {
    host.create_agent(
        AgentCreator::System {
            component: "memory-fixture".to_owned(),
        },
        AgentCreationOrigin::Provisioning,
        AgentCreateRequest::from_preset(
            Uuid::new_v4(),
            AgentPresetId::new(preset).expect("preset"),
            name,
        )
        .with_instructions("Answer."),
        CancellationToken::new(),
    )
    .await
    .expect("agent")
    .id
}

async fn session(host: &LocalHost, root: &Path, agent: renoa_kernel::AgentId) -> Arc<AgentSession> {
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace");
    host.ensure_agent_session(agent, &workspace, Uuid::new_v4())
        .await
        .expect("session")
}

async fn turn(
    session: &AgentSession,
    command: Uuid,
    speaker: Speaker,
    prompt: &str,
) -> Result<LocalTurnOutcome, LocalHostError> {
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
}

async fn say(session: &AgentSession, speaker: Speaker, prompt: &str) -> String {
    match turn(session, Uuid::new_v4(), speaker, prompt)
        .await
        .unwrap_or_else(|error| panic!("{prompt} failed: {error}"))
    {
        LocalTurnOutcome::Completed { output, .. } => output,
        other => panic!("{prompt} did not complete: {other:?}"),
    }
}

fn data(root: &Path) -> std::path::PathBuf {
    root.join("data")
}

fn notes(output: &str) -> Vec<Value> {
    let value: Value = serde_json::from_str(output).unwrap_or_else(|_| panic!("{output}"));
    value["notes"].as_array().expect("notes").clone()
}

#[tokio::test]
async fn a_persons_notes_are_shared_by_every_agent_that_talks_to_them_and_no_one_else() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let host = host(root);
    let arcee = session(
        &host,
        root,
        agent(&host, crate::presets::APP_PRESET_ID, "Arcee").await,
    )
    .await;
    let scout = session(
        &host,
        root,
        agent(&host, crate::presets::GENERAL_PRESET_ID, "[Scout]").await,
    )
    .await;
    let ravi_id = Uuid::new_v4();
    let (asha, ravi) = (
        Speaker::Principal(Uuid::new_v4()),
        Speaker::Principal(ravi_id),
    );

    let written: Value = serde_json::from_str(
        &say(
            &arcee,
            asha,
            "note: Dentist on Friday at 10 [on 2026-11-06]",
        )
        .await,
    )
    .expect("note result");
    assert_eq!(written, json!({"id": 0}));
    let seen = notes(&say(&scout, asha, "recall: dentist").await);
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0]["author"], "Arcee");
    assert_eq!(seen[0]["text"], "Dentist on Friday at 10 [on 2026-11-06]");
    assert_eq!(seen[0]["happened"], "2026-11-06");
    say(&scout, asha, "note: [updates #0] Dentist moved to Monday").await;
    let corrected = notes(&say(&arcee, asha, "recall: dentist").await);
    assert_eq!(corrected[0]["superseded_by"], 1);
    assert_eq!(
        corrected[1]["author"], "Scout",
        "a bracketed name signs without brackets"
    );
    let zoomed: Value = serde_json::from_str(&say(&arcee, asha, "zoom: 0-1").await).expect("zoom");
    assert_eq!(
        zoomed["entries"].as_array().map(Vec::len),
        Some(2),
        "{zoomed}"
    );

    assert!(
        notes(&say(&arcee, ravi, "recall: .").await).is_empty(),
        "Ravi has his own notebook"
    );
    assert!(
        !data(root).join("users").join(ravi_id.to_string()).exists(),
        "a read creates nothing"
    );
    assert!(
        say(&arcee, asha, "note: x [on 2026-02-30]")
            .await
            .starts_with("error: "),
        "Mnem's own check reaches the agent"
    );
}

#[tokio::test]
async fn a_guest_or_unidentified_turn_has_no_notebook_and_no_mind() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let host = host(root);
    let owner = Uuid::new_v4();
    fixtures::dream(
        &fixtures::notebook(&data(root), owner),
        "Training for a 10k",
    );
    let arcee = session(
        &host,
        root,
        agent(&host, crate::presets::APP_PRESET_ID, "Arcee").await,
    )
    .await;
    assert_ne!(say(&arcee, Speaker::Principal(owner), "show").await, "none");

    for speaker in [Speaker::Unidentified, Speaker::Guest] {
        let refused = say(&arcee, speaker, "note: Lives on Mars").await;
        assert!(refused.starts_with("error: "), "{speaker:?}: {refused}");
        assert!(
            say(&arcee, speaker, "recall: .")
                .await
                .starts_with("error: ")
        );
        assert_eq!(
            say(&arcee, speaker, "show").await,
            "none",
            "{speaker:?} gets no MIND.md"
        );
    }
    let users: Vec<String> = fs::read_dir(data(root).join("users"))
        .expect("users")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(users, [owner.to_string()], "nothing was created for them");
    let refusal = say(&arcee, Speaker::Unidentified, "note: x").await;
    assert!(
        refusal.contains("No person is identified"),
        "the refusal says why: {refusal}"
    );
    assert!(
        refusal.contains("tell them you cannot keep notes for them here"),
        "and what to tell the person, as on Telegram and Slack: {refusal}"
    );
}

/// An agent keeping time in `zone`, with `renoa.time` off unless `clock`.
async fn zoned(
    host: &LocalHost,
    root: &Path,
    name: &str,
    zone: &str,
    clock: bool,
) -> Arc<AgentSession> {
    let id = agent(host, crate::presets::GENERAL_PRESET_ID, name).await;
    settings::configure(
        &host.config.database,
        id,
        HostPluginId::Time,
        json!({"timezone": zone}),
        name,
    )
    .expect("zone");
    if !clock {
        state::change(
            &host.config.database,
            id,
            HostPluginId::Time,
            false,
            "clock-off",
        )
        .expect("off");
    }
    session(host, root, id).await
}

/// `format` of the current time in `zone`.
fn now_in(zone: &str, format: &str) -> String {
    jiff::fmt::strtime::format(format, &jiff::Timestamp::now().in_tz(zone).expect("zone"))
        .expect("format")
}

#[tokio::test]
async fn notes_are_dated_in_the_main_agents_zone_whoever_writes_them() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let host = host(root);
    let person = Speaker::Principal(Uuid::new_v4());
    // Twenty-five hours apart, so their dates always differ, and the Host
    // zone's date can match only one. The main agent, made first, takes the
    // other, so its date is neither the writer's nor the Host's.
    let host_today = jiff::Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::try_system().unwrap_or(jiff::tz::TimeZone::UTC))
        .date()
        .to_string();
    let (main, other) = if now_in("Pacific/Kiritimati", "%Y-%m-%d") == host_today {
        ("Pacific/Pago_Pago", "Pacific/Kiritimati")
    } else {
        ("Pacific/Kiritimati", "Pacific/Pago_Pago")
    };
    let east = zoned(&host, root, "East", main, true).await;
    let west = zoned(&host, root, "West", other, true).await;
    let before = now_in(main, "%Y-%m-%d");
    say(&east, person, "note: Written by East").await;
    say(&west, person, "note: Written by West").await;
    let after = now_in(main, "%Y-%m-%d");
    let found = notes(&say(&west, person, "recall: written by").await);
    assert_eq!(found.len(), 2);
    for note in &found {
        let written = note["written"].as_str().expect("written");
        assert!(
            written == before || written == after,
            "{note} not in {before}..{after}, the main agent's dates"
        );
    }
}

#[tokio::test]
async fn an_agent_whose_clock_is_not_the_notebooks_is_told_the_notebooks_time() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let host = host(root);
    let person = Speaker::Principal(Uuid::new_v4());
    let main = zoned(&host, root, "Arcee", "Asia/Kolkata", true).await;
    let helper = zoned(&host, root, "Helper", "UTC", true).await;
    let twin = zoned(&host, root, "Twin", "Asia/Kolkata", true).await;
    let clockless = zoned(&host, root, "Clockless", "Asia/Kolkata", false).await;
    let entry = "Memory times are in Asia/Kolkata; it is now ";

    let before = now_in("Asia/Kolkata", "%Y-%m-%d %H:%M");
    let told = say(&helper, person, "context").await;
    let after = now_in("Asia/Kolkata", "%Y-%m-%d %H:%M");
    let shown = told
        .split(entry)
        .nth(1)
        .and_then(|rest| rest.get(..16))
        .unwrap_or_else(|| panic!("the helper in UTC is told IST: {told}"));
    assert!(
        shown == before || shown == after,
        "{shown} is the time in Kolkata ({before}..{after})"
    );
    assert!(
        say(&clockless, person, "context").await.contains(entry),
        "an agent without a clock is told"
    );
    for (name, session) in [("the main agent", &main), ("a twin", &twin)] {
        let context = say(session, person, "context").await;
        assert!(
            !context.contains(entry),
            "{name} already shows it: {context}"
        );
    }
    assert!(
        !say(&helper, Speaker::Unidentified, "context")
            .await
            .contains(entry),
        "a turn with no person has no notebook"
    );
}

fn block_revision(block: &str) -> &str {
    block
        .split("revision=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no revision in {block}"))
}

fn skipped(host: &LocalHost, session: &AgentSession) -> Vec<String> {
    let trace = host
        .config
        .sessions
        .join(session.id().to_string())
        .join(crate::trace::TRACE_DATABASE);
    rusqlite::Connection::open(trace)
        .expect("trace")
        .prepare("SELECT payload_json FROM events WHERE kind = 'session_context_skipped'")
        .expect("query")
        .query_map([], |row| row.get(0))
        .expect("rows")
        .collect::<Result<_, _>>()
        .expect("payloads")
}

#[tokio::test]
async fn mind_enters_the_prompt_after_user_md_and_follows_its_revision() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let host = host(root);
    let owner = Uuid::new_v4();
    let person = Speaker::Principal(owner);
    let id = agent(&host, crate::presets::APP_PRESET_ID, "Arcee").await;
    let arcee = session(&host, root, id).await;
    assert_eq!(
        say(&arcee, person, "show").await,
        "none",
        "no MIND.md, no block"
    );

    let notebook = fixtures::notebook(&data(root), owner);
    let first = fixtures::dream(&notebook, "Training for a 10k");
    let prompt = say(&arcee, person, "prompt").await;
    let (user, block) = (
        prompt.find("<user source=\"USER.md\"").expect("USER.md"),
        prompt.find("<session_context>").expect("block"),
    );
    assert!(user < block, "the block follows USER.md: {prompt}");
    let shown = say(&arcee, person, "show").await;
    assert_eq!(block_revision(&shown), first);
    assert!(shown.contains("Training for a 10k"), "{shown}");
    assert_eq!(
        say(&arcee, person, "show").await,
        shown,
        "an unchanged MIND.md keeps its bytes"
    );

    let second = fixtures::dream(&notebook, "Recovering from a sprain");
    assert_eq!(
        block_revision(&say(&arcee, person, "show").await),
        second,
        "a new revision reloads"
    );

    state::change(
        &host.config.database,
        id,
        HostPluginId::Memory,
        false,
        "memory-off",
    )
    .expect("off");
    assert_eq!(
        say(&arcee, person, "show").await,
        "none",
        "a disabled plugin adds no block"
    );
    state::change(
        &host.config.database,
        id,
        HostPluginId::Memory,
        true,
        "memory-on",
    )
    .expect("on");

    for largest in fixtures::largest_prose_minds() {
        fs::write(notebook.join("MIND.md"), largest).expect("the largest MIND.md");
        assert_ne!(
            say(&arcee, person, "show").await,
            "none",
            "the main agent's whole view of ordinary prose fits"
        );
    }
    fs::write(notebook.join("MIND.md"), fixtures::escaping_mind()).expect("an escaping MIND.md");
    assert_eq!(
        say(&arcee, person, "show").await,
        "none",
        "one that escapes past the bound is left out, and the turn runs"
    );

    fs::write(notebook.join("MIND.md"), "not a mind").expect("corrupt MIND.md");
    assert_eq!(
        say(&arcee, person, "show").await,
        "none",
        "the turn runs without the block"
    );
    let oversized = format!(
        "## Durable\n- {} (#0)\n## Active\n## Hunches\n## Nudges\n",
        "x".repeat(40_000)
    );
    fs::write(notebook.join("MIND.md"), oversized).expect("oversized MIND.md");
    assert_eq!(say(&arcee, person, "show").await, "none");
    let reports = skipped(&host, &arcee);
    assert_eq!(
        reports.len(),
        3,
        "escaping, corrupt, oversized: {reports:?}"
    );
    assert!(
        reports.iter().all(|report| report.contains("renoa.memory")),
        "{reports:?}"
    );
}

/// Waits until `path` exists, bounded so a broken fixture fails the test.
async fn appears(path: &Path) {
    for _ in 0..1_000 {
        if path.exists() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("{} never appeared", path.display());
}

#[tokio::test]
async fn a_recovered_command_keeps_the_block_it_was_admitted_with() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let host = host(root);
    let owner = Uuid::new_v4();
    let person = Speaker::Principal(owner);
    let arcee = session(
        &host,
        root,
        agent(&host, crate::presets::APP_PRESET_ID, "Arcee").await,
    )
    .await;
    let notebook = fixtures::notebook(&data(root), owner);
    let first = fixtures::dream(&notebook, "Training for a 10k");

    // The model call is in flight when the turn is dropped, as a crash leaves it.
    let command = Uuid::new_v4();
    let started = root.join("started");
    fs::write(root.join("hang"), "").expect("hang");
    tokio::select! {
        outcome = turn(&arcee, command, person, "show") => panic!("the turn did not hang: {outcome:?}"),
        () = appears(&started) => {}
    }
    fs::remove_file(root.join("hang")).expect("release");
    let second = fixtures::dream_with_nudge(
        &notebook,
        "Recovering from a sprain",
        Some("Ask about the ankle"),
    );
    // Arcee, made first, was the main agent; the owner names another.
    let scout = agent(&host, crate::presets::APP_PRESET_ID, "Scout").await;
    rusqlite::Connection::open(&host.config.database)
        .expect("catalog")
        .execute(
            "UPDATE host_main_agent SET agent_id = ?1, revision = revision + 1",
            [scout.to_string()],
        )
        .expect("a new main agent");
    assert_ne!(
        second,
        shared_revision(root, owner),
        "the main agent's view differs"
    );

    let recovered = loop {
        match turn(&arcee, command, person, "show").await {
            Ok(LocalTurnOutcome::Completed { output, .. }) => break output,
            // The dropped model call releases the session once it is cleaned up.
            Err(error) if error.to_string().contains("active driver") => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            other => panic!("the recovered command failed: {other:?}"),
        }
    };
    assert_eq!(
        block_revision(&recovered),
        first,
        "the frozen prompt is rebuilt"
    );
    assert_eq!(
        block_revision(&say(&arcee, person, "show").await),
        shared_revision(root, owner),
        "the next command reloads, as an agent that is not the main one"
    );
}

/// The revision of the person's MIND.md as every agent but the main one sees
/// it: without its Nudges.
fn shared_revision(root: &Path, owner: Uuid) -> String {
    crate::plugins::host::memory::mind(&data(root), owner)
        .expect("mind")
        .expect("written")
        .without_nudges()
        .revision
}

#[tokio::test]
async fn only_the_main_agent_sees_the_nudges() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let host = host(root);
    let owner = Uuid::new_v4();
    let person = Speaker::Principal(owner);
    // The first agent made on a Host is its main agent.
    let arcee = session(
        &host,
        root,
        agent(&host, crate::presets::APP_PRESET_ID, "Arcee").await,
    )
    .await;
    let scout = session(
        &host,
        root,
        agent(&host, crate::presets::APP_PRESET_ID, "Scout").await,
    )
    .await;
    let notebook = fixtures::notebook(&data(root), owner);
    let full = fixtures::dream_with_nudge(
        &notebook,
        "Training for a 10k",
        Some("Ask how the long run went"),
    );

    let main = say(&arcee, person, "show").await;
    assert_eq!(block_revision(&main), full);
    assert!(main.contains("Ask how the long run went"), "{main}");
    let handed = "before it is handed to you";
    assert!(
        main.contains(handed),
        "the main agent is told how nudges arrive"
    );
    let other = say(&scout, person, "show").await;
    assert!(other.contains("Training for a 10k"), "{other}");
    assert!(!other.contains("Ask how the long run went"), "{other}");
    assert!(!other.contains(handed), "{other}");
    assert_eq!(block_revision(&other), shared_revision(root, owner));

    fixtures::dream_with_nudge(&notebook, "Training for a 10k", Some("Suggest a rest day"));
    let changed = say(&arcee, person, "show").await;
    assert_ne!(
        block_revision(&changed),
        full,
        "the main agent's view moves"
    );
    assert!(changed.contains("Suggest a rest day"), "{changed}");
    assert_eq!(
        say(&scout, person, "show").await,
        other,
        "a nudge-only change leaves every other agent's block as it was"
    );
}

#[tokio::test]
async fn every_preset_starts_with_memory_and_schema_43_admits_it_for_existing_agents() {
    let directory = tempfile::tempdir().expect("directory");
    let host = host(directory.path());
    let database = host.config.database.clone();
    let mut agents = Vec::new();
    for preset in crate::presets::catalog() {
        let id = agent(&host, preset.id().as_str(), "Preset").await;
        assert!(
            state::enabled(&database, id, HostPluginId::Memory).expect("enabled"),
            "{}",
            preset.id().as_str()
        );
        agents.push(id);
    }

    let connection = rusqlite::Connection::open(&database).expect("catalog");
    crate::host::catalog::restore_schema_42(&connection);
    connection
        .execute_batch(
            "UPDATE host_metadata SET schema_version = 42 WHERE singleton = 1;
             PRAGMA user_version = 42;",
        )
        .expect("store schema 42");
    let refused = connection.execute(
        "INSERT INTO host_agent_builtin_plugins(agent_id, plugin_id, enabled) VALUES (?1, 'renoa.memory', 0)",
        [agents[0].to_string()],
    );
    assert!(refused.is_err(), "schema 42 does not know renoa.memory");
    let rows_before: i64 = connection
        .query_row(
            "SELECT count(*) FROM host_agent_builtin_plugins",
            [],
            |row| row.get(0),
        )
        .expect("rows");
    drop(connection);

    crate::host::catalog::initialize(&database).expect("upgrade schema 42");
    crate::host::catalog::initialize(&database).expect("reopen the upgraded catalog");
    let rows: i64 = rusqlite::Connection::open(&database)
        .expect("catalog")
        .query_row(
            "SELECT count(*) FROM host_agent_builtin_plugins",
            [],
            |row| row.get(0),
        )
        .expect("rows");
    assert_eq!(
        rows, rows_before,
        "every stored activation is kept, none added"
    );
    for id in &agents {
        assert!(state::enabled(&database, *id, HostPluginId::Memory).expect("enabled"));
    }
    let coding = agents
        .iter()
        .zip(crate::presets::catalog())
        .find(|(_, preset)| preset.id().as_str() == crate::presets::ALPHA_PRESET_ID)
        .map(|(id, _)| *id)
        .expect("the coding agent");
    assert!(
        !state::enabled(&database, coding, HostPluginId::Time).expect("time"),
        "renoa.time stays off"
    );
    state::change(
        &database,
        agents[0],
        HostPluginId::Memory,
        false,
        "memory-off",
    )
    .expect("the CHECK admits it");
}

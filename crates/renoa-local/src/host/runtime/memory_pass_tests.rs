//! `renoa.memory`'s note pass through the real scheduler, conversation
//! sessions and kernel, with a scripted model and an injected clock.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use jiff::Timestamp;
use mnem::memory::Memory;
use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock};
use renoa_kernel::AgentId;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, AgentSession,
    AutomationScheduler, HostObserver, LocalHostError, LocalTurnOutcome, ModelProvider, RunStatus,
    ScheduledRun, Speaker, TurnObservation,
    agent_session::claim::SURFACE_WAIT,
    host::{HostInitialization, LocalHost},
    plugins::host::{
        HostPluginId,
        memory::{
            fixtures,
            note_pass::{self, clock},
            pass,
        },
        settings, state,
    },
};

/// A conversation turn answers `ok`, or, for a message with [`HANG`] in it,
/// writes `started` and never answers; compaction and Dream answer as in the
/// nightly tests. A pass takes the steps in `script.json` beside it, in
/// order: `note:<text>` calls `memory_note`, `call:<name> <json>` calls that
/// tool, `say:<text>` answers, and `hang` writes `started` and never answers;
/// with no step left it answers `NONE`. Every call is logged to
/// `calls.jsonl` with its system prompt, tools and messages. While a `broken`
/// file is beside it, the model cannot be listed or loaded.
const MODEL: &str = r#"
import { createHash } from "node:crypto";
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
const spec = JSON.stringify({id:"fixture"});
const here = name => new URL("./" + name, import.meta.url);
const HANG = "Hang on forever, please.";
if (process.env.RENOA_MODEL_ACTION !== "stream" && existsSync(here("broken"))) {
  console.log(JSON.stringify({ok:false,error:"the model is gone"}));
} else if (process.env.RENOA_MODEL_ACTION === "catalog") {
  console.log(JSON.stringify({ok:true,response:{models:[{id:"fixture",name:"Fixture",reasoning_levels:["high"],context_window_tokens:500000,model_spec:{id:"fixture"}}]}}));
} else if (process.env.RENOA_MODEL_ACTION === "describe") {
  console.log(JSON.stringify({ok:true,response:{context_window_tokens:500000,max_output_tokens:32768,model_spec:spec,model_binding_id:createHash("sha256").update(spec).digest("hex"),reasoning_level:"high"}}));
} else {
  let input=""; for await (const part of process.stdin) input+=part;
  const request=JSON.parse(input);
  const complete=(content,stop_reason="stop")=>console.log(JSON.stringify({event:"completed",response:{content,stop_reason,usage:{input:10,output:1,cache_read:0,cache_write:0},metadata:{api:"fixture",provider:"xai",model:"fixture"}}}));
  const say=text=>complete(text===""?[]:[{type:"text",text}]);
  const call=(id,name,args)=>complete([{type:"tool_call",id,name,arguments:args}],"tool_use");
  const index=request.messages.findLastIndex(message=>message.role==="user");
  const prompt=request.messages[index].content[0].text;
  const results=request.messages.slice(index+1).filter(message=>message.role==="tool").map(message=>message.result);
  const kind=prompt.startsWith("Task: write one line")?"compaction":prompt.startsWith("Task: you are the reflective")?"dream":prompt.includes("<already-noted>")?"pass":"turn";
  appendFileSync(here("calls.jsonl"), JSON.stringify({kind, system:request.system_prompt, tools:JSON.stringify(request.tools), messages:request.messages, prompt, results})+"\n");
  if (kind==="compaction") say("Walks the dog at dawn");
  else if (kind==="dream") {
    const [first]=prompt.match(/Notes #(\d+) to/).slice(1);
    say(`<reflection>Nothing new.</reflection>\n<mind>\n## Durable\n## Active\n- Walks the dog at dawn (#${first})\n## Hunches\n## Nudges\n</mind>\n`);
  } else if (kind==="turn" && JSON.stringify(request.messages[index]).includes(HANG)) {
    writeFileSync(here("started"), ""); setInterval(()=>{}, 1000);
  } else if (kind==="turn") say("ok");
  else {
    const steps=existsSync(here("script.json"))?JSON.parse(readFileSync(here("script.json"),"utf8")):[];
    const step=steps.shift();
    writeFileSync(here("script.json"), JSON.stringify(steps));
    if (step===undefined) say("NONE");
    else if (step==="hang") { writeFileSync(here("started"), ""); setInterval(()=>{}, 1000); }
    else if (step.startsWith("note:")) call("note"+results.length, "memory_note", {text:step.slice(5)});
    else if (step.startsWith("call:")) {
      const space=step.indexOf(" ");
      call("call"+results.length, step.slice(5, space), JSON.parse(step.slice(space+1)));
    } else if (step.startsWith("say:")) say(step.slice(4));
    else say("unknown step");
  }
}
"#;

/// 12:00 on 2026-11-02 in Asia/Kolkata, the notebook zone.
pub(super) const NOON: &str = "2026-11-02T06:30:00Z";
/// 11:00 on 2026-11-03 in Asia/Kolkata, Mnem's pass day start: the nightly
/// run, and the end of the pass day 2026-11-02.
const NIGHT: &str = "2026-11-03T05:30:00Z";
pub(super) const MINUTE: i64 = 60_000;
/// More than Mnem's least unreviewed text for a pass.
pub(super) const ENOUGH: &str =
    "My sister lands on Friday at ten and I promised to pick her up from the airport.";
const TIMER: &str = "renoa.memory note pass";
/// A message whose turn the model never answers.
const HANG: &str = "Hang on forever, please.";

pub(super) fn ms(at: &str) -> i64 {
    at.parse::<Timestamp>()
        .expect("a test instant")
        .as_millisecond()
}

pub(super) struct Quiet;

impl AgentEventSink for Quiet {
    fn emit(&self, _: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

pub(super) struct Fixture {
    _directory: tempfile::TempDir,
    pub(super) root: PathBuf,
    pub(super) host: LocalHost,
    pub(super) scheduler: AutomationScheduler,
    pub(super) main: AgentId,
}

impl Fixture {
    pub(super) async fn new() -> Self {
        let directory = tempfile::tempdir().expect("directory");
        let root = directory.path().to_path_buf();
        fs::write(root.join("model.mjs"), MODEL).expect("model");
        fs::write(root.join("auth.sqlite"), "").expect("auth boundary");
        fs::create_dir_all(root.join("workspace")).expect("workspace");
        let host = assemble(&root);
        let main = agent(&host, "Arcee").await;
        settings::configure(
            &host.config.database,
            main,
            HostPluginId::Time,
            json!({"timezone": "Asia/Kolkata"}),
            "zone",
        )
        .expect("the main agent's zone");
        let scheduler = host.automation_scheduler().expect("scheduler");
        Self {
            _directory: directory,
            root,
            host,
            scheduler,
            main,
        }
    }

    /// This fixture's Host stopped and assembled again on its data, with the
    /// model's spec changed, so every runtime it resolves differs from the
    /// stopped Host's: a deploy.
    fn redeployed(self) -> Self {
        let Self {
            _directory: directory,
            root,
            host,
            scheduler,
            main,
        } = self;
        drop(scheduler);
        drop(host);
        let model = MODEL.replace(
            r#"JSON.stringify({id:"fixture"})"#,
            r#"JSON.stringify({id:"fixture",revision:2})"#,
        );
        assert_ne!(model, MODEL);
        fs::write(root.join("model.mjs"), model).expect("model");
        let host = assemble(&root);
        let scheduler = host.automation_scheduler().expect("scheduler");
        Self {
            _directory: directory,
            root,
            host,
            scheduler,
            main,
        }
    }

    pub(super) fn database(&self) -> &Path {
        &self.host.config.database
    }

    pub(super) async fn open(&self, agent: AgentId, session: Uuid) -> Arc<AgentSession> {
        self.host
            .ensure_agent_session(agent, &self.root.join("workspace"), session)
            .await
            .expect("session")
    }

    /// `person` writes `text` to `agent` in `session` at `at`, and the turn
    /// ends then.
    async fn say_to(&self, agent: AgentId, session: Uuid, person: Uuid, at: i64, text: &str) {
        clock::set(self.database(), at);
        let opened = self.open(agent, session).await;
        let outcome = opened
            .execute_turn_observed_with_cancellation(
                Uuid::new_v4(),
                vec![ContentBlock::text(text)],
                TurnObservation::from_unix_milliseconds(at).expect("observation"),
                Arc::new(Quiet),
                CancellationToken::new(),
                Speaker::Principal(person),
            )
            .await
            .expect("the turn runs");
        assert!(
            matches!(&outcome, LocalTurnOutcome::Completed { output, .. } if output == "ok"),
            "{outcome:?}"
        );
    }

    pub(super) async fn say(&self, session: Uuid, person: Uuid, at: i64, text: &str) {
        self.say_to(self.main, session, person, at, text).await;
    }

    /// Admits the plugin run due at `now`, if any, runs it and records it.
    async fn run(&self, now: i64) -> Option<(ScheduledRun, RunStatus)> {
        let due = self
            .scheduler
            .next_run(now, false)
            .await
            .expect("next run")?;
        assert_eq!(due.plugin, Some("renoa.memory"));
        let finished = self
            .scheduler
            .run_for_plugin(&self.host, &due, now)
            .await
            .expect("the run ends");
        let status = finished.status();
        self.scheduler
            .finish_plugin_run(finished, now)
            .await
            .expect("record the run");
        if status != RunStatus::Succeeded {
            let summary = self
                .snapshot()
                .await
                .automations
                .into_iter()
                .find(|automation| automation.id == due.run.automation_id)
                .and_then(|automation| automation.last_run)
                .and_then(|run| run.summary);
            eprintln!("the run ended {status:?}: {summary:?}");
        }
        Some((due, status))
    }

    /// Runs the agent's automation due at `at`, as the node does, and returns
    /// which automation it was.
    async fn agent_run(&self, at: i64) -> Uuid {
        let due = self
            .scheduler
            .next_run(at, false)
            .await
            .expect("next run")
            .expect("a run is due");
        assert_eq!(due.plugin, None, "the agent's, not a plugin's");
        self.scheduler
            .finish_run(
                due.run.id,
                crate::RunOutcome::Succeeded {
                    answer: "Done.".to_owned(),
                    failed_tool_calls: 0,
                    landing: crate::RunLanding::OwnTask,
                },
                at,
            )
            .await
            .expect("record the run");
        due.run.automation_id
    }

    /// The note pass timer's run due at `now`.
    async fn timer(&self, now: i64) -> Option<RunStatus> {
        let (due, status) = self.run(now).await?;
        assert_eq!(
            due.run.automation_id,
            crate::plugins::host::memory::note_pass::timer_id(self.main),
            "the timer, not the nightly run"
        );
        Some(status)
    }

    /// When the timer is next due, as inspection shows it once the scheduler
    /// converged it at `now`, when no run is due.
    async fn timer_due(&self, now: i64) -> Option<i64> {
        assert!(
            self.scheduler
                .next_run(now, false)
                .await
                .expect("next run")
                .is_none(),
            "no run is due"
        );
        self.snapshot()
            .await
            .automations
            .into_iter()
            .find(|automation| automation.name == TIMER && automation.enabled)
            .map(|automation| automation.next_due_ms)
    }

    pub(super) async fn snapshot(&self) -> crate::HostObservation {
        HostObserver::open(&self.root.join("data"))
            .expect("observer")
            .snapshot()
            .await
            .expect("snapshot")
    }

    fn script(&self, steps: &[&str]) {
        fs::write(self.root.join("script.json"), json!(steps).to_string()).expect("script");
    }

    fn calls(&self) -> Vec<Value> {
        fs::read_to_string(self.root.join("calls.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("a logged call"))
            .collect()
    }

    fn calls_of(&self, kind: &str) -> Vec<Value> {
        self.calls()
            .into_iter()
            .filter(|call| call["kind"] == kind)
            .collect()
    }

    /// A notebook for `person` with `count` notes, enough for Dream.
    fn notebook(&self, person: Uuid, count: u64) {
        let memory = Memory::create(&fixtures::notebook(&self.root.join("data"), person))
            .expect("a notebook");
        for n in 0..count {
            memory
                .note(
                    "Fixture",
                    &format!("Walked the dog at dawn, day {n}"),
                    "2026-11-01T07:00".parse().expect("a time"),
                )
                .expect("a note");
        }
    }

    fn notes(&self, person: Uuid) -> u64 {
        let directory = fixtures::notebook(&self.root.join("data"), person);
        if !directory.exists() {
            return 0;
        }
        Memory::open_read_only(&directory)
            .expect("notebook")
            .len()
            .expect("length")
    }

    /// Each pass, oldest first: its conversation, outcome, notes and day.
    fn passes(&self) -> Vec<(Uuid, Option<String>, Option<i64>, String)> {
        rusqlite::Connection::open(self.database())
            .expect("catalog")
            .prepare(
                "SELECT session_id, outcome, noted, pass_day FROM host_memory_passes
                 ORDER BY frozen_at_ms, command_id",
            )
            .expect("passes")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?.parse().expect("id"),
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                ))
            })
            .expect("passes")
            .collect::<Result<_, _>>()
            .expect("passes")
    }

    /// The note pass events logged for this fixture's agent.
    fn events(&self, name: &str) -> Vec<Value> {
        pass::EVENTS
            .lock()
            .expect("events")
            .iter()
            .filter(|(_, event, fields)| *event == name && fields["agent_id"] == json!(self.main))
            .map(|(_, _, fields)| fields.clone())
            .collect()
    }
}

fn assemble(root: &Path) -> LocalHost {
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

/// `agent`'s automation `name`, once at `at`.
async fn once(host: &LocalHost, agent: AgentId, name: &str, at: &str) -> Uuid {
    host.manage_automation(
        agent,
        Uuid::new_v4(),
        crate::AutomationMutation::Create {
            spec: crate::AutomationSpec {
                agent_id: agent,
                name: name.to_owned(),
                prompt: format!("Remind them: {name}"),
                schedule: crate::AutomationSchedule::Once { at: at.to_owned() },
                enabled: true,
            },
        },
        ms(NOON),
        CancellationToken::new(),
    )
    .await
    .expect("an automation")
    .id
}

async fn agent(host: &LocalHost, name: &str) -> AgentId {
    host.create_agent(
        AgentCreator::System {
            component: "memory-pass-fixture".to_owned(),
        },
        AgentCreationOrigin::Provisioning,
        AgentCreateRequest::from_preset(
            Uuid::new_v4(),
            AgentPresetId::new(crate::presets::APP_PRESET_ID).expect("preset"),
            name,
        )
        .with_instructions("Answer as Arcee."),
        CancellationToken::new(),
    )
    .await
    .expect("agent")
    .id
}

/// The text of every user message `call` sent, in order.
fn user_texts(call: &Value) -> Vec<String> {
    call["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter(|message| message["role"] == "user")
        .map(|message| {
            message["content"]
                .as_array()
                .expect("content")
                .iter()
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect()
}

#[tokio::test]
async fn a_quiet_conversation_gets_a_pass_that_notes_and_later_turns_see_only_its_receipt() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.say(session, person, noon, ENOUGH).await;

    assert!(
        fixture.timer(noon + 14 * MINUTE).await.is_none(),
        "not quiet yet"
    );
    assert_eq!(
        fixture.timer_due(noon + 14 * MINUTE).await,
        Some(noon + 15 * MINUTE)
    );
    fixture.script(&[
        "note:Picks up her sister from the airport on Friday at 10",
        "say:#0",
    ]);
    assert_eq!(
        fixture.timer(noon + 15 * MINUTE).await,
        Some(RunStatus::Succeeded)
    );
    assert_eq!(fixture.notes(person), 1, "the pass noted");
    let passes = fixture.passes();
    assert_eq!(
        passes,
        [(
            session,
            Some("completed".to_owned()),
            Some(1),
            "2026-11-02".to_owned()
        )]
    );
    assert_eq!(
        fixture.timer_due(noon + 15 * MINUTE).await,
        None,
        "nothing unreviewed is left"
    );

    fixture
        .say(session, person, noon + 20 * MINUTE, "Thanks.")
        .await;
    let calls = fixture.calls();
    let kinds: Vec<&str> = calls
        .iter()
        .map(|call| call["kind"].as_str().expect("kind"))
        .collect();
    assert_eq!(kinds, ["turn", "pass", "pass", "turn"]);
    let (before, pass, after) = (&calls[0], &calls[1], &calls[3]);
    assert!(
        pass["prompt"]
            .as_str()
            .expect("prompt")
            .contains("Review the whole conversation."),
        "a conversation the pass followed from its start is reviewed whole"
    );
    assert_eq!(pass["system"], before["system"], "the cached prefix holds");
    assert_eq!(pass["tools"], before["tools"]);
    assert_eq!(after["system"], before["system"]);
    assert_eq!(after["tools"], before["tools"]);
    let seen = user_texts(after);
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert!(seen[0].contains(ENOUGH));
    assert_eq!(
        seen[1],
        note_pass::shown_receipt("noted #0"),
        "the pass shows as its receipt"
    );
    assert!(seen[2].contains("Thanks."));
    let shown = serde_json::to_string(&after["messages"]).expect("messages");
    assert!(!shown.contains("<already-noted>") && !shown.contains("airport on Friday"));
    let history = fixture
        .open(fixture.main, session)
        .await
        .history()
        .expect("history");
    let replayed: Vec<&renoa_agent::Message> = history.iter().map(|entry| &entry.message).collect();
    let replayed = serde_json::to_string(&replayed).expect("history");
    assert!(
        replayed.contains("Thanks.") && !replayed.contains("<already-noted>"),
        "a surface replays no pass: {replayed}"
    );

    let completed = fixture.events("memory_pass");
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0]["outcome"], "completed");
    assert_eq!(completed[0]["noted"], 1);
    let observed = fixture.snapshot().await;
    assert_eq!(observed.memory_passes.len(), 1);
    assert_eq!(observed.memory_passes[0].principal_id, person);
    assert_eq!(observed.memory_passes[0].passes_today, 1);
    let inspected = serde_json::to_string(&observed).expect("inspection");
    assert!(
        !inspected.contains("airport"),
        "inspection shows no note text"
    );
}

#[tokio::test]
async fn no_pass_runs_in_a_turn_within_the_gap_or_for_too_little_text() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);

    fixture.say(session, person, noon, "Hi there.").await;
    assert_eq!(
        fixture.timer_due(noon).await,
        None,
        "too little text to ask again"
    );
    assert!(fixture.timer(noon + 5 * MINUTE).await.is_none());

    fixture
        .say(session, person, noon + 6 * MINUTE, ENOUGH)
        .await;
    // A turn running in the conversation holds its claim.
    let held = fixture
        .host
        .config
        .claims
        .surface_uncancelled(session)
        .await
        .expect("a turn's claim");
    assert_eq!(
        fixture.timer(noon + 21 * MINUTE).await,
        Some(RunStatus::Succeeded)
    );
    assert!(
        fixture.calls_of("pass").is_empty(),
        "no pass while a turn holds it"
    );
    let waited = fixture.events("memory_pass_decision");
    assert_eq!(
        waited.last().expect("a decision")["reason"],
        "a turn holds it"
    );
    assert_eq!(
        fixture.timer_due(noon + 21 * MINUTE).await,
        Some(noon + 36 * MINUTE)
    );
    drop(held);

    assert_eq!(
        fixture.timer(noon + 36 * MINUTE).await,
        Some(RunStatus::Succeeded)
    );
    assert_eq!(fixture.calls_of("pass").len(), 1);

    fixture
        .say(session, person, noon + 40 * MINUTE, ENOUGH)
        .await;
    assert_eq!(
        fixture.timer_due(noon + 40 * MINUTE).await,
        Some(noon + 156 * MINUTE),
        "two hours after the last pass"
    );
    assert!(fixture.timer(noon + 60 * MINUTE).await.is_none());
    assert_eq!(fixture.calls_of("pass").len(), 1);
    assert_eq!(
        fixture.timer(noon + 156 * MINUTE).await,
        Some(RunStatus::Succeeded)
    );
    assert_eq!(fixture.calls_of("pass").len(), 2);
}

#[tokio::test]
async fn the_daytime_cap_leaves_the_last_pass_of_the_day_to_the_sweep_before_dream() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let (late, other) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.notebook(person, 5);
    let call = once(
        &fixture.host,
        fixture.main,
        "Call mom",
        "2026-11-02T19:00:00+05:30",
    )
    .await;
    for pass in 0..4 {
        let at = noon + pass * 2 * 60 * MINUTE;
        fixture.say(session, person, at, ENOUGH).await;
        assert_eq!(
            fixture.timer(at + 15 * MINUTE).await,
            Some(RunStatus::Succeeded)
        );
    }
    assert_eq!(fixture.calls_of("pass").len(), 4);
    // The promised call is made at 19:00, in the day the sweep ends.
    assert_eq!(fixture.agent_run(ms("2026-11-02T13:30:00Z")).await, call);
    // 09:00 on the next morning is still the same pass day.
    fixture
        .say(session, person, ms(NIGHT) - 2 * 60 * MINUTE, ENOUGH)
        .await;
    assert_eq!(
        fixture.timer_due(ms(NIGHT) - 2 * 60 * MINUTE).await,
        None,
        "the daytime passes are spent"
    );
    let spent = fixture.events("memory_pass_decision");
    assert_eq!(spent.len(), 4, "every decision was to run");
    // Someone still talking at 10:50 is left by the sweep.
    fixture
        .say(late, other, ms(NIGHT) - 10 * MINUTE, ENOUGH)
        .await;
    fixture.script(&["note:Picks up her sister on Friday", "say:#0"]);

    let (due, status) = fixture.run(ms(NIGHT)).await.expect("the nightly run");
    assert_ne!(
        due.run.automation_id,
        crate::plugins::host::memory::note_pass::timer_id(fixture.main)
    );
    assert_eq!(status, RunStatus::Succeeded);
    let kinds: Vec<String> = fixture
        .calls()
        .iter()
        .map(|call| call["kind"].as_str().expect("kind").to_owned())
        .collect();
    let sweep = kinds
        .iter()
        .rposition(|kind| kind == "pass")
        .expect("the sweep");
    let dream = kinds
        .iter()
        .position(|kind| kind == "dream")
        .expect("a dream");
    assert!(sweep < dream, "the sweep runs before Dream: {kinds:?}");
    let swept = fixture.calls()[sweep]["prompt"]
        .as_str()
        .expect("prompt")
        .to_owned();
    let (_, ended) = swept
        .split_once("Finished or deleted since 2026-11-02T11:00:")
        .expect("the ended automations");
    assert!(
        ended.contains("· Call mom ·"),
        "the sweep lists what ended in its day: {ended}"
    );
    let passes = fixture.passes();
    assert_eq!(passes.len(), 5);
    assert!(
        passes.iter().all(|(at, outcome, _, day)| *at == session
            && outcome.as_deref() == Some("completed")
            && day == "2026-11-02"),
        "the sweep counts toward the day the night ends: {passes:?}"
    );
    let decisions = fixture.events("memory_pass_decision");
    let left: Vec<&Value> = decisions
        .iter()
        .filter(|decision| decision["session_id"] == json!(late))
        .collect();
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0]["decision"], "wait");
    assert_eq!(left[0]["kind"], "sweep");
}

#[tokio::test]
async fn a_refused_call_during_a_pass_errors_with_what_to_do_and_changes_nothing() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.say(session, person, noon, ENOUGH).await;
    let soul = r#"call:agent_documents {"document":"soul","expected_revision":"0000000000000000000000000000000000000000000000000000000000000000","content":"Replaced."}"#;
    let create = r#"call:tool_execute {"reference":"host:renoa.automations:v1:automation_manage","arguments":{"action":"create","name":"Airport","prompt":"Remind me","schedule":{"kind":"once","at":"2026-11-06T04:00:00Z"}}}"#;
    let zoom = r#"call:memory_zoom {"block":"0"}"#;
    fixture.script(&[soul, create, zoom, "say:NONE"]);
    assert_eq!(
        fixture.timer(noon + 15 * MINUTE).await,
        Some(RunStatus::Succeeded)
    );
    let last = fixture
        .calls_of("pass")
        .pop()
        .expect("the pass's last call");
    let results = last["results"].as_array().expect("results");
    assert_eq!(results.len(), 3);
    for result in results {
        assert_eq!(result["is_error"], true, "{result}");
        let text = result["content"][0]["text"].as_str().expect("text");
        assert!(text.contains("This is a memory pass"), "{text}");
    }
    assert!(
        results[0]["content"][0]["text"]
            .as_str()
            .expect("text")
            .contains("USER.md")
    );
    assert!(
        results[1]["content"][0]["text"]
            .as_str()
            .expect("text")
            .contains("[at YYYY-MM-DDTHH:MM]")
    );
    let observed = fixture.snapshot().await;
    assert!(
        observed
            .automations
            .iter()
            .all(|automation| automation.plugin.is_some()),
        "no automation was created"
    );
    let completed = fixture.events("memory_pass");
    assert_eq!(completed[0]["failed_tool_calls"], 3);
    let soul = fs::read_to_string(
        fixture
            .host
            .home()
            .path()
            .join("agents")
            .join(fixture.main.to_string())
            .join("SOUL.md"),
    )
    .unwrap_or_default();
    assert!(!soul.contains("Replaced."));
}

#[tokio::test]
async fn a_message_during_a_pass_cancels_it_and_is_admitted_once_it_settles() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.say(session, person, noon, ENOUGH).await;
    fixture.script(&["note:Picks up her sister on Friday", "hang"]);
    let started = fixture.root.join("started");
    let back = async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let asked = Instant::now();
        fixture
            .say(session, person, noon + 20 * MINUTE, "I'm back.")
            .await;
        asked.elapsed()
    };
    let (status, waited) = tokio::time::timeout(
        Duration::from_secs(60),
        Box::pin(async { tokio::join!(fixture.timer(noon + 15 * MINUTE), back) }),
    )
    .await
    .expect("the message cancels the pass");
    assert_eq!(status, Some(RunStatus::Succeeded));
    assert!(
        waited < Duration::from_secs(10),
        "admitted promptly: {waited:?}"
    );
    assert_eq!(fixture.notes(person), 1, "the note stays");
    assert_eq!(
        fixture.passes(),
        [(
            session,
            Some("cancelled".to_owned()),
            Some(1),
            "2026-11-02".to_owned()
        )]
    );
    assert_eq!(fixture.events("memory_pass_cancelled").len(), 1);
    assert_eq!(fixture.snapshot().await.memory_passes[0].passes_today, 0);
    let seen = user_texts(fixture.calls_of("turn").last().expect("the reply"));
    assert_eq!(
        seen[1],
        note_pass::shown_receipt("stopped early, noted #0")
    );
}

#[tokio::test]
async fn a_pass_a_crash_left_is_closed_without_counting_or_noting_twice() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.say(session, person, noon, ENOUGH).await;
    fixture.script(&["note:Picks up her sister on Friday", "hang"]);
    let started = fixture.root.join("started");
    let crash = async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::select! {
        _ = fixture.timer(noon + 15 * MINUTE) => panic!("the pass hangs"),
        () = crash => {}
        () = tokio::time::sleep(Duration::from_secs(60)) => panic!("the pass never started"),
    }
    assert_eq!(fixture.notes(person), 1);
    assert_eq!(fixture.passes()[0].1, None, "the pass is unsettled");

    // The run is handed on again; it closes the pass and runs a new one.
    fixture.script(&["say:#0"]);
    assert_eq!(
        fixture.timer(noon + 16 * MINUTE).await,
        Some(RunStatus::Succeeded)
    );
    assert_eq!(fixture.notes(person), 1, "the note is not written again");
    let passes = fixture.passes();
    let outcomes: Vec<Option<&str>> = passes.iter().map(|pass| pass.1.as_deref()).collect();
    assert_eq!(outcomes, [Some("cancelled"), Some("completed")]);
    assert_eq!(fixture.snapshot().await.memory_passes[0].passes_today, 1);
    let calls = fixture.calls_of("pass");
    assert_eq!(calls.len(), 3, "the closed pass made no model call");
    assert!(
        calls[2]["prompt"]
            .as_str()
            .expect("prompt")
            .contains("Picks up her sister on Friday"),
        "the next pass lists what was noted"
    );
}

#[tokio::test]
async fn conversations_pass_most_unreviewed_first_and_share_the_persons_cap() {
    let fixture = Fixture::new().await;
    let person = Uuid::new_v4();
    let (short, long) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.say(short, person, noon, ENOUGH).await;
    fixture
        .say(long, person, noon, &format!("{ENOUGH} {ENOUGH}"))
        .await;
    assert_eq!(
        fixture.timer(noon + 15 * MINUTE).await,
        Some(RunStatus::Succeeded)
    );
    let ran = fixture.events("memory_pass");
    assert_eq!(ran.len(), 1, "one pass, then the gap");
    assert_eq!(ran[0]["session_id"], json!(long), "most unreviewed first");
    let decisions = fixture.events("memory_pass_decision");
    let waiting = decisions.last().expect("the second conversation");
    assert_eq!(waiting["session_id"], json!(short));
    assert_eq!(waiting["decision"], "wait");
    assert_eq!(
        fixture.timer_due(noon + 15 * MINUTE).await,
        Some(noon + 15 * MINUTE + 2 * 60 * MINUTE)
    );
}

#[tokio::test]
async fn no_pass_follows_another_agent_or_runs_with_the_plugin_off() {
    let fixture = Fixture::new().await;
    let person = Uuid::new_v4();
    let noon = ms(NOON);
    let scout = agent(&fixture.host, "Scout").await;
    fixture
        .say_to(scout, Uuid::new_v4(), person, noon, ENOUGH)
        .await;
    assert_eq!(fixture.timer_due(noon).await, None, "only the main agent's");
    let followed: i64 = rusqlite::Connection::open(fixture.database())
        .expect("catalog")
        .query_row(
            "SELECT count(*) FROM host_memory_conversations",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(followed, 0, "another agent's conversation is not followed");

    let session = Uuid::new_v4();
    fixture.say(session, person, noon, ENOUGH).await;
    assert_eq!(fixture.timer_due(noon).await, Some(noon + 15 * MINUTE));
    state::change(
        fixture.database(),
        fixture.main,
        HostPluginId::Memory,
        false,
        "off",
    )
    .expect("off");
    assert!(fixture.timer(noon + 15 * MINUTE).await.is_none());
    fixture
        .say(session, person, noon + 20 * MINUTE, ENOUGH)
        .await;
    assert!(fixture.timer(noon + 60 * MINUTE).await.is_none());
    assert!(fixture.calls_of("pass").is_empty());
}

#[tokio::test]
async fn schema_45_adds_the_note_pass_tables_to_a_schema_44_catalog() {
    let fixture = Fixture::new().await;
    let database = fixture.database().to_path_buf();
    assert!(
        fixture
            .scheduler
            .next_run(ms(NOON), false)
            .await
            .expect("next")
            .is_none()
    );
    let connection = rusqlite::Connection::open(&database).expect("catalog");
    crate::host::catalog::restore_schema_44(&connection);
    connection
        .execute_batch(
            "UPDATE host_metadata SET schema_version = 44 WHERE singleton = 1;
             PRAGMA user_version = 44;",
        )
        .expect("store schema 44");
    drop(connection);
    crate::host::catalog::initialize(&database).expect("upgrade schema 44");
    crate::host::catalog::initialize(&database).expect("reopen the upgraded catalog");

    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.say(session, person, noon, ENOUGH).await;
    fixture.script(&["note:Picks up her sister on Friday", "say:#0"]);
    assert_eq!(
        fixture.timer(noon + 15 * MINUTE).await,
        Some(RunStatus::Succeeded),
        "the conversation and pass tables are in place"
    );
    assert_eq!(fixture.notes(person), 1);
}

#[tokio::test]
async fn a_conversation_a_surface_keeps_open_between_turns_gets_its_pass() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    // Kept open, as a surface's cache keeps it.
    let kept = fixture.open(fixture.main, session).await;
    fixture.say(session, person, noon, ENOUGH).await;
    assert!(
        Arc::ptr_eq(&kept, &fixture.open(fixture.main, session).await),
        "every opener shares the one open instance"
    );
    fixture.script(&["note:Picks up her sister on Friday", "say:#0"]);
    assert_eq!(
        fixture.timer(noon + 15 * MINUTE).await,
        Some(RunStatus::Succeeded)
    );
    assert_eq!(
        fixture.passes(),
        [(
            session,
            Some("completed".to_owned()),
            Some(1),
            "2026-11-02".to_owned()
        )],
        "the pass ran while the surface kept the conversation open"
    );
    let outcome = kept
        .execute_turn_observed_with_cancellation(
            Uuid::new_v4(),
            vec![ContentBlock::text("Thanks.")],
            TurnObservation::from_unix_milliseconds(noon + 20 * MINUTE).expect("observation"),
            Arc::new(Quiet),
            CancellationToken::new(),
            Speaker::Principal(person),
        )
        .await
        .expect("the kept instance takes the next turn");
    assert!(matches!(outcome, LocalTurnOutcome::Completed { .. }));
}

#[tokio::test]
async fn a_stopping_host_settles_its_pass_so_the_redeployed_one_takes_the_next_message() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.say(session, person, noon, ENOUGH).await;
    fixture.script(&["note:Picks up her sister on Friday", "hang"]);
    let started = fixture.root.join("started");
    let stop = async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        fixture.host.stop_memory_runs();
    };
    let (status, ()) = tokio::time::timeout(
        Duration::from_secs(60),
        Box::pin(async { tokio::join!(fixture.timer(noon + 15 * MINUTE), stop) }),
    )
    .await
    .expect("stopping ends the pass and its run");
    assert_eq!(status, Some(RunStatus::Succeeded));
    assert_eq!(
        fixture.passes(),
        [(
            session,
            Some("cancelled".to_owned()),
            Some(1),
            "2026-11-02".to_owned()
        )],
        "settled as not run, its note kept"
    );
    let cancelled = fixture.events("memory_pass_cancelled");
    assert_eq!(
        cancelled.last().expect("a cancellation")["reason"],
        "the Host is stopping"
    );

    let fixture = fixture.redeployed();
    fixture
        .say(session, person, noon + 20 * MINUTE, "I'm back.")
        .await;
    assert_eq!(fixture.notes(person), 1);
    assert_eq!(fixture.snapshot().await.memory_passes[0].passes_today, 0);
}

#[tokio::test]
async fn an_agents_due_run_is_handed_on_while_a_pass_runs() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.say(session, person, noon, ENOUGH).await;
    let call = once(
        &fixture.host,
        fixture.main,
        "Call mom",
        "2026-11-02T12:16:00+05:30",
    )
    .await;
    fixture.script(&["hang"]);
    let now = noon + 16 * MINUTE;
    let pass = fixture
        .scheduler
        .next_run(now, false)
        .await
        .expect("next run")
        .expect("the timer is due first");
    assert_eq!(pass.plugin, Some("renoa.memory"));
    let started = fixture.root.join("started");
    let beside = async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let due = fixture
            .scheduler
            .next_run(now, true)
            .await
            .expect("next run");
        let reported = match &due {
            Some(due) => {
                fixture
                    .scheduler
                    .finish_run(
                        due.run.id,
                        crate::RunOutcome::Succeeded {
                            answer: "Reminded.".to_owned(),
                            failed_tool_calls: 0,
                            landing: crate::RunLanding::OwnTask,
                        },
                        now,
                    )
                    .await
                    .expect("record the call");
                fixture
                    .host
                    .completed_automation_runs(0)
                    .await
                    .expect("completed runs")
                    .iter()
                    .any(|run| run.id == due.run.id)
            }
            None => false,
        };
        fixture.host.stop_memory_runs();
        (due, reported)
    };
    let (finished, (due, reported)) = tokio::time::timeout(
        Duration::from_secs(60),
        Box::pin(async {
            tokio::join!(
                fixture.scheduler.run_for_plugin(&fixture.host, &pass, now),
                beside
            )
        }),
    )
    .await
    .expect("the pass ends");
    fixture
        .scheduler
        .finish_plugin_run(finished.expect("the pass's run ends"), now)
        .await
        .expect("record the pass's run");
    let due = due.expect("the call is handed on while the pass runs");
    assert_eq!((due.run.automation_id, due.plugin), (call, None));
    assert!(
        reported,
        "its result reaches surfaces while the pass is unfinished"
    );
}

#[tokio::test]
async fn deleting_a_conversation_during_its_pass_cancels_it_and_leaves_nothing_of_it() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.say(session, person, noon, ENOUGH).await;
    let scout = agent(&fixture.host, "Scout").await;
    assert!(
        fixture.host.delete_session(scout, session).await.is_err(),
        "only its own agent deletes it"
    );
    assert_eq!(rows(&fixture), (1, 0), "a refused deletion removes nothing");
    fixture.script(&["note:Picks up her sister on Friday", "hang"]);
    let started = fixture.root.join("started");
    let delete = async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(fixture.host.delete_session(scout, session).await.is_err());
        assert_eq!(
            fixture.passes()[0].1,
            None,
            "a refused deletion leaves the pass running"
        );
        fixture.host.delete_session(fixture.main, session).await
    };
    let (status, deleted) = tokio::time::timeout(
        Duration::from_secs(60),
        Box::pin(async { tokio::join!(fixture.timer(noon + 15 * MINUTE), delete) }),
    )
    .await
    .expect("the deletion cancels the pass");
    assert_eq!(status, Some(RunStatus::Succeeded));
    deleted.expect("deleted once the pass settled");
    assert!(!fixture.host.has_session(session).expect("sessions"));
    assert_eq!(rows(&fixture), (0, 0), "nothing of it is left");
    assert_eq!(fixture.notes(person), 1, "the person's note stays theirs");
}

#[tokio::test]
async fn a_conversation_with_turns_before_the_pass_followed_it_is_reviewed_from_then() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture
        .open(fixture.main, session)
        .await
        .execute_turn(
            Uuid::new_v4(),
            vec![ContentBlock::text("We spoke before the pass was here.")],
            Arc::new(Quiet),
        )
        .await
        .expect("an earlier turn");
    fixture.say(session, person, noon, ENOUGH).await;
    assert_eq!(
        fixture.timer(noon + 15 * MINUTE).await,
        Some(RunStatus::Succeeded)
    );
    let prompt = fixture.calls_of("pass")[0]["prompt"]
        .as_str()
        .expect("prompt")
        .to_owned();
    assert!(
        prompt.contains("Review the conversation after the last memory-pass receipt (since")
            && !prompt.contains("Review the whole conversation."),
        "{prompt}"
    );
}

/// How many conversations and passes the note pass keeps.
fn rows(fixture: &Fixture) -> (i64, i64) {
    let db = rusqlite::Connection::open(fixture.database()).expect("catalog");
    let count = |table: &str| {
        db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("count")
    };
    (
        count("host_memory_conversations"),
        count("host_memory_passes"),
    )
}

#[tokio::test]
async fn inspecting_a_conversation_during_its_pass_cancels_the_pass_first() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.say(session, person, noon, ENOUGH).await;
    fixture.script(&["hang"]);
    let started = fixture.root.join("started");
    let inspect = async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        fixture
            .host
            .inspect_session(fixture.main, session, &fixture.root.join("workspace"))
            .await
            .map(|history| history.history().expect("history").len())
    };
    let (status, inspected) = tokio::time::timeout(
        Duration::from_secs(60),
        Box::pin(async { tokio::join!(fixture.timer(noon + 15 * MINUTE), inspect) }),
    )
    .await
    .expect("the inspection cancels the pass");
    assert_eq!(status, Some(RunStatus::Succeeded));
    assert_eq!(inspected.expect("inspected"), 2, "the turn, not the pass");
    assert_eq!(fixture.passes()[0].1.as_deref(), Some("cancelled"));
}

#[tokio::test]
async fn a_stopping_host_starts_no_job_of_a_nightly_run() {
    let fixture = Fixture::new().await;
    let person = Uuid::new_v4();
    fixture.notebook(person, 5);
    // Scheduled for 11:00 the next day, at noon.
    assert_eq!(fixture.timer_due(ms(NOON)).await, None);
    fixture.host.stop_memory_runs();
    let (_, status) = fixture.run(ms(NIGHT)).await.expect("the nightly run");
    assert_eq!(status, RunStatus::Failed, "stopped part way");
    assert!(
        fixture.calls().is_empty(),
        "no compaction or Dream starts once the Host is stopping"
    );
}

/// The person's unreviewed characters the pass counts in `session`.
fn unreviewed(fixture: &Fixture, session: Uuid) -> i64 {
    rusqlite::Connection::open(fixture.database())
        .expect("catalog")
        .query_row(
            "SELECT user_chars FROM host_memory_conversations WHERE session_id = ?1",
            [session.to_string()],
            |row| row.get(0),
        )
        .expect("the conversation")
}

/// `person`'s message `text` under `request` at `at`, as a surface sends it.
async fn send(
    opened: &AgentSession,
    request: Uuid,
    (person, at): (Uuid, i64),
    text: &str,
    cancellation: CancellationToken,
) -> Result<LocalTurnOutcome, LocalHostError> {
    opened
        .execute_turn_observed_with_cancellation(
            request,
            vec![ContentBlock::text(text)],
            TurnObservation::from_unix_milliseconds(at).expect("observation"),
            Arc::new(Quiet),
            cancellation,
            Speaker::Principal(person),
        )
        .await
}

fn answered(outcome: &Result<LocalTurnOutcome, LocalHostError>) -> bool {
    matches!(outcome, Ok(LocalTurnOutcome::Completed { output, .. }) if output == "ok")
}

#[tokio::test]
async fn a_message_the_kernel_never_admitted_counts_for_nothing_and_its_resend_counts_once() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.say(session, person, noon, ENOUGH).await;
    let counted = unreviewed(&fixture, session);
    let opened = fixture.open(fixture.main, session).await;
    let (request, text) = (Uuid::new_v4(), "I'm back from the airport.");
    let broken = fixture.root.join("broken");
    fs::write(&broken, "").expect("break the model");
    let refused = send(
        &opened,
        request,
        (person, noon + MINUTE),
        text,
        CancellationToken::new(),
    )
    .await;
    assert!(refused.is_err(), "no runtime, so no turn: {refused:?}");
    assert_eq!(
        unreviewed(&fixture, session),
        counted,
        "a message the kernel never admitted counts for nothing"
    );
    fs::remove_file(&broken).expect("mend the model");
    for _ in 0..2 {
        let sent = send(
            &opened,
            request,
            (person, noon + MINUTE),
            text,
            CancellationToken::new(),
        )
        .await;
        assert!(answered(&sent), "{sent:?}");
    }
    assert_eq!(
        unreviewed(&fixture, session),
        counted + i64::try_from(text.chars().count()).expect("a count"),
        "sent again, it counts once"
    );
}

#[tokio::test]
async fn a_pass_that_does_not_let_go_refuses_a_message_and_holds_an_automation_run_until_it_does() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.say(session, person, noon, ENOUGH).await;
    let call = once(
        &fixture.host,
        fixture.main,
        "Call mom",
        "2026-11-02T12:16:00+05:30",
    )
    .await;
    let opened = fixture.open(fixture.main, session).await;
    // A pass that does not stop when it is cancelled.
    let held = fixture
        .host
        .config
        .claims
        .note_pass(session, &CancellationToken::new())
        .expect("a free conversation");
    let refused = send(
        &opened,
        Uuid::new_v4(),
        (person, noon + MINUTE),
        "I'm back.",
        CancellationToken::new(),
    )
    .await;
    assert!(
        matches!(refused, Err(LocalHostError::PassStillRunning)),
        "{refused:?}"
    );
    let due = fixture
        .scheduler
        .next_run(noon + 16 * MINUTE, true)
        .await
        .expect("next run")
        .expect("the call is due");
    assert_eq!(due.run.automation_id, call);
    // The node delivers the run as a command under the run's own id.
    let deliver = opened.execute_turn_observed_with_cancellation(
        due.run.id,
        vec![ContentBlock::text(due.run.submission.clone())],
        TurnObservation::from_unix_milliseconds(noon + 16 * MINUTE).expect("observation"),
        Arc::new(Quiet),
        CancellationToken::new(),
        Speaker::Unidentified,
    );
    let release = async move {
        tokio::time::sleep(SURFACE_WAIT * 5 / 2).await;
        drop(held);
    };
    let (delivered, ()) = tokio::time::timeout(
        Duration::from_secs(60),
        Box::pin(async { tokio::join!(deliver, release) }),
    )
    .await
    .expect("the run is delivered once the pass lets go");
    assert!(answered(&delivered), "{delivered:?}");
    let delayed = pass::EVENTS
        .lock()
        .expect("events")
        .iter()
        .filter(|(_, event, fields)| {
            *event == "memory_pass_delays_command" && fields["command_id"] == json!(due.run.id)
        })
        .count();
    assert!(delayed >= 2, "it waited past the bound: {delayed}");
    assert_eq!(
        fixture.calls_of("turn").len(),
        2,
        "the first message and the reminder ran; the refused one did not"
    );
}

#[tokio::test]
async fn cancelling_a_turn_that_waits_for_a_pass_runs_nothing() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    let opened = fixture.open(fixture.main, session).await;
    let first = Uuid::new_v4();
    let said = send(
        &opened,
        first,
        (person, noon),
        ENOUGH,
        CancellationToken::new(),
    )
    .await;
    assert!(answered(&said), "{said:?}");
    let held = fixture
        .host
        .config
        .claims
        .note_pass(session, &CancellationToken::new())
        .expect("a free conversation");
    // ACP's cancel reaches a turn waiting for its claim.
    let waiting = send(
        &opened,
        Uuid::new_v4(),
        (person, noon + MINUTE),
        "Never mind.",
        CancellationToken::new(),
    );
    let cancel = async {
        tokio::time::sleep(SURFACE_WAIT / 4).await;
        opened.cancel_active_turn().expect("cancel");
    };
    let (outcome, ()) = tokio::join!(waiting, cancel);
    assert!(
        matches!(outcome, Ok(LocalTurnOutcome::Cancelled)),
        "{outcome:?}"
    );
    // So does the caller's own cancellation.
    let token = CancellationToken::new();
    let waiting = send(
        &opened,
        Uuid::new_v4(),
        (person, noon + MINUTE),
        "Never mind.",
        token.clone(),
    );
    let cancel = async {
        tokio::time::sleep(SURFACE_WAIT / 4).await;
        token.cancel();
    };
    let (outcome, ()) = tokio::join!(waiting, cancel);
    assert!(
        matches!(outcome, Ok(LocalTurnOutcome::Cancelled)),
        "{outcome:?}"
    );
    // A message the kernel already admitted settles only under the claim:
    // cancelled, it waits on past the bound and is settled as the kernel
    // has it, here replayed.
    let token = CancellationToken::new();
    let waiting = send(&opened, first, (person, noon), ENOUGH, token.clone());
    let cancel_then_release = async {
        tokio::time::sleep(SURFACE_WAIT / 4).await;
        token.cancel();
        tokio::time::sleep(SURFACE_WAIT * 2).await;
        drop(held);
    };
    let (outcome, ()) = tokio::join!(waiting, cancel_then_release);
    assert!(answered(&outcome), "{outcome:?}");
    // With no pass holding it, a cancelled caller still takes the claim, and
    // the kernel answers for a message it already settled.
    let token = CancellationToken::new();
    token.cancel();
    let replayed = send(&opened, first, (person, noon), ENOUGH, token).await;
    assert!(answered(&replayed), "{replayed:?}");
    assert_eq!(fixture.calls_of("turn").len(), 1, "none of them ran");
}

#[tokio::test]
async fn deleting_a_conversation_keeps_its_completed_pass_toward_the_persons_gap() {
    let fixture = Fixture::new().await;
    let (session, later, person) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.say(session, person, noon, ENOUGH).await;
    assert_eq!(
        fixture.timer(noon + 15 * MINUTE).await,
        Some(RunStatus::Succeeded)
    );
    assert_eq!(fixture.passes()[0].1.as_deref(), Some("completed"));
    fixture
        .host
        .delete_session(fixture.main, session)
        .await
        .expect("deleted");
    assert_eq!(rows(&fixture), (0, 1), "its completed pass stays");
    fixture.say(later, person, noon + 20 * MINUTE, ENOUGH).await;
    assert_eq!(
        fixture.timer_due(noon + 20 * MINUTE).await,
        Some(noon + 135 * MINUTE),
        "two hours after the deleted conversation's pass"
    );
    // Once past the time a pass is kept, a deletion prunes it.
    clock::set(fixture.database(), noon + 3 * 24 * 60 * MINUTE);
    fixture
        .host
        .delete_session(fixture.main, later)
        .await
        .expect("deleted");
    assert_eq!(rows(&fixture), (0, 0));
}

#[tokio::test]
async fn deleting_a_conversation_a_surface_holds_open_is_refused_without_cancelling_its_pass() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.say(session, person, noon, ENOUGH).await;
    let opened = fixture.open(fixture.main, session).await;
    fixture.script(&["hang"]);
    let started = fixture.root.join("started");
    let delete = async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let refused = fixture.host.delete_session(fixture.main, session).await;
        let running = fixture.passes()[0].1.clone();
        drop(opened);
        let deleted = fixture.host.delete_session(fixture.main, session).await;
        (refused, running, deleted)
    };
    let (status, (refused, running, deleted)) = tokio::time::timeout(
        Duration::from_secs(60),
        Box::pin(async { tokio::join!(fixture.timer(noon + 15 * MINUTE), delete) }),
    )
    .await
    .expect("the deletion ends");
    assert!(
        refused
            .as_ref()
            .is_err_and(|error| error.to_string().contains("close it first")),
        "{refused:?}"
    );
    assert_eq!(running, None, "the refused deletion left the pass running");
    deleted.expect("deleted once the surface let go");
    assert_eq!(status, Some(RunStatus::Succeeded));
    assert!(!fixture.host.has_session(session).expect("sessions"));
}

#[tokio::test]
async fn deleting_a_conversation_whose_pass_a_crash_left_removes_the_pass_and_its_message() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.say(session, person, noon, ENOUGH).await;
    fixture.script(&["hang"]);
    let started = fixture.root.join("started");
    let crash = async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::select! {
        _ = fixture.timer(noon + 15 * MINUTE) => panic!("the pass hangs"),
        () = crash => {}
        () = tokio::time::sleep(Duration::from_secs(60)) => panic!("the pass never started"),
    }
    assert_eq!(fixture.passes()[0].1, None, "the pass is unsettled");
    fixture
        .host
        .delete_session(fixture.main, session)
        .await
        .expect("deleted");
    assert_eq!(
        rows(&fixture),
        (0, 0),
        "an unsettled pass goes with its conversation, frozen message and all"
    );
}

#[tokio::test]
async fn a_cancelled_message_the_kernel_admitted_waits_for_the_pass_and_settles() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.say(session, person, noon, ENOUGH).await;
    let opened = fixture.open(fixture.main, session).await;
    let request = Uuid::new_v4();
    // Admitted, then left unfinished, as a stopped process leaves a turn.
    let started = fixture.root.join("started");
    let admitted = async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::select! {
        _ = send(&opened, request, (person, noon + MINUTE), HANG, CancellationToken::new()) => {
            panic!("the turn hangs")
        }
        () = admitted => {}
        () = tokio::time::sleep(Duration::from_secs(60)) => panic!("the turn never started"),
    }
    let held = fixture
        .host
        .config
        .claims
        .note_pass(session, &CancellationToken::new())
        .expect("a free conversation");
    let token = CancellationToken::new();
    let waiting = send(
        &opened,
        request,
        (person, noon + MINUTE),
        HANG,
        token.clone(),
    );
    let cancel_then_release = async {
        tokio::time::sleep(SURFACE_WAIT / 4).await;
        token.cancel();
        tokio::time::sleep(SURFACE_WAIT * 2).await;
        drop(held);
    };
    let (outcome, ()) = tokio::time::timeout(
        Duration::from_secs(60),
        Box::pin(async { tokio::join!(waiting, cancel_then_release) }),
    )
    .await
    .expect("the command settles once the pass lets go");
    assert!(
        matches!(outcome, Ok(LocalTurnOutcome::Cancelled)),
        "{outcome:?}"
    );
    drop(opened);
    // Nothing is left unfinished, so the next message is admitted.
    fixture
        .say(session, person, noon + 2 * MINUTE, "Hello again.")
        .await;
}

#[tokio::test]
async fn a_message_whose_delivery_cannot_be_read_waits_for_the_pass() {
    let fixture = Fixture::new().await;
    let (session, person) = (Uuid::new_v4(), Uuid::new_v4());
    let noon = ms(NOON);
    fixture.say(session, person, noon, ENOUGH).await;
    let opened = fixture.open(fixture.main, session).await;
    let held = fixture
        .host
        .config
        .claims
        .note_pass(session, &CancellationToken::new())
        .expect("a free conversation");
    // Whether a command is a run being delivered cannot be read.
    let db = rusqlite::Connection::open(fixture.database()).expect("catalog");
    db.execute_batch("ALTER TABLE host_automation_runs RENAME TO host_automation_runs_gone")
        .expect("hide the runs");
    let waiting = send(
        &opened,
        Uuid::new_v4(),
        (person, noon + MINUTE),
        "I'm back.",
        CancellationToken::new(),
    );
    let release = async {
        tokio::time::sleep(SURFACE_WAIT * 5 / 2).await;
        // In one step, so the waiting turn runs with the runs back.
        drop(held);
        db.execute_batch("ALTER TABLE host_automation_runs_gone RENAME TO host_automation_runs")
            .expect("restore the runs");
    };
    let (outcome, ()) = tokio::time::timeout(
        Duration::from_secs(60),
        Box::pin(async { tokio::join!(waiting, release) }),
    )
    .await
    .expect("the message runs once the pass lets go");
    assert!(answered(&outcome), "{outcome:?}");
    let unread = pass::EVENTS
        .lock()
        .expect("events")
        .iter()
        .filter(|(_, event, fields)| {
            *event == "memory_pass_delivery_unread" && fields["session_id"] == json!(session)
        })
        .count();
    assert!(unread >= 2, "each lap is logged: {unread}");
}

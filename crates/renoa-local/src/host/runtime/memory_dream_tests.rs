//! `renoa.memory`'s nightly run through the real scheduler, memory session
//! and kernel, with a scripted model and an injected clock.

use std::{fs, path::Path};

use jiff::{Timestamp, civil::DateTime};
use mnem::{
    dream::{self, Trigger},
    memory::Memory,
    model::Answer,
};
use renoa_kernel::AgentId;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, AutomationMutation,
    AutomationScheduler, HostAutomationControl, HostObserver, ModelProvider, RunStatus,
    ScheduledRun,
    host::{HostInitialization, LocalHost},
    plugins::host::{
        HostPluginId,
        memory::{fixtures, pass},
        settings, state,
    },
    runtime::MEMORY_JOB_INSTRUCTIONS,
};

mod replies;
mod stuck;

/// Answers a compaction with a line and a dream with a `MIND.md` citing the
/// first new note, unless `script.json` beside it holds steps, which it
/// takes in order: `say:<text>`, `cut:<text>` (stopped at the output limit),
/// `silent` (no text), `echo` (the prompt back) or `fail` (a provider error).
/// While a `broken` file is beside it, the model cannot be listed or loaded.
/// Every call is logged to `calls.jsonl`.
const MODEL: &str = r#"
import { createHash } from "node:crypto";
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
const spec = JSON.stringify({id:"fixture"});
if (process.env.RENOA_MODEL_ACTION !== "stream" && existsSync(new URL("./broken", import.meta.url))) {
  console.log(JSON.stringify({ok:false,error:"the model is gone"}));
} else if (process.env.RENOA_MODEL_ACTION === "catalog") {
  console.log(JSON.stringify({ok:true,response:{models:[{id:"fixture",name:"Fixture",reasoning_levels:["high"],context_window_tokens:500000,model_spec:{id:"fixture"}}]}}));
} else if (process.env.RENOA_MODEL_ACTION === "describe") {
  console.log(JSON.stringify({ok:true,response:{context_window_tokens:500000,max_output_tokens:32768,model_spec:spec,model_binding_id:createHash("sha256").update(spec).digest("hex"),reasoning_level:"high"}}));
} else {
  let input=""; for await (const part of process.stdin) input+=part;
  const request=JSON.parse(input);
  const prompt=request.messages.at(-1).content[0].text;
  const kind=prompt.startsWith("Task: write one line")?"compaction":prompt.startsWith("Task: you are the reflective")?"dream":"other";
  appendFileSync(new URL("./calls.jsonl", import.meta.url), JSON.stringify({
    kind, system:request.system_prompt, tools:request.tools.length, messages:request.messages.length,
    turn_context:JSON.stringify(request.messages).includes("<turn_context>"),
    now:prompt.match(/Now: (\S+)/)?.[1] ?? null, notes:prompt.match(/Notes #(\d+) to #(\d+), written/)?.slice(1) ?? null,
    profile:prompt.includes("<user-profile>"), block:prompt.match(/summarizes notes #(\S+) of/)?.[1] ?? null,
  })+"\n");
  const script=new URL("./script.json", import.meta.url);
  const steps=existsSync(script)?JSON.parse(readFileSync(script,"utf8")):[];
  const step=steps.shift();
  writeFileSync(script, JSON.stringify(steps));
  const say=(text,stop_reason="stop")=>console.log(JSON.stringify({event:"completed",response:{content:text===""?[]:[{type:"text",text}],stop_reason,usage:{input:10,output:1,cache_read:0,cache_write:0},metadata:{api:"fixture",provider:"xai",model:"fixture"}}}));
  if (step==="fail") console.log(JSON.stringify({event:"error",error:"the provider is unreachable",error_kind:"invalid_request",inference_outcome:"known_not_started"}));
  else if (step==="silent") say("");
  else if (step==="echo") say(prompt);
  else if (step?.startsWith("say:")) say(step.slice(4));
  else if (step?.startsWith("cut:")) say(step.slice(4), "length");
  else if (kind==="compaction") say("Walks the dog at dawn");
  else if (kind==="dream") {
    const [first]=prompt.match(/Notes #(\d+) to/).slice(1);
    say(`<reflection>Nothing new.</reflection>\n<mind>\n## Durable\n## Active\n- Walks the dog at dawn (#${first})\n## Hunches\n## Nudges\n</mind>\n`);
  } else say("unknown");
}
"#;

/// 2026-11-02T12:00Z, a Monday afternoon: the night after is the first one.
const NOON: &str = "2026-11-02T12:00:00Z";
/// 11:00 on 2026-11-03 in Asia/Kolkata, the notebook zone.
const FIRST_NIGHT: &str = "2026-11-03T05:30:00Z";

fn ms(at: &str) -> i64 {
    at.parse::<Timestamp>()
        .expect("a test instant")
        .as_millisecond()
}

fn at(raw: &str) -> DateTime {
    raw.parse().expect("a test time")
}

struct Fixture {
    _directory: tempfile::TempDir,
    root: std::path::PathBuf,
    host: LocalHost,
    scheduler: AutomationScheduler,
    main: AgentId,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().expect("directory");
        let root = directory.path().to_path_buf();
        fs::write(root.join("model.mjs"), MODEL).expect("model");
        fs::write(root.join("auth.sqlite"), "").expect("auth boundary");
        let host = LocalHost::assemble(HostInitialization {
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
        .expect("host");
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

    fn data(&self) -> std::path::PathBuf {
        self.root.join("data")
    }

    fn database(&self) -> &Path {
        &self.host.config.database
    }

    /// A notebook for `principal` with `count` notes, and their `USER.md`.
    fn notebook(&self, principal: Uuid, count: u64) -> Memory {
        let directory = fixtures::notebook(&self.data(), principal);
        let memory = Memory::create(&directory).expect("a notebook");
        add_notes(&memory, count);
        fs::write(
            directory.parent().expect("person").join("USER.md"),
            "Lives in Pune.\n",
        )
        .expect("USER.md");
        memory
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

    fn kinds(&self) -> Vec<String> {
        self.calls()
            .iter()
            .map(|call| call["kind"].as_str().expect("kind").to_owned())
            .collect()
    }

    async fn next(&self, now: i64) -> Option<ScheduledRun> {
        self.scheduler.next_run(now).await.expect("next run")
    }

    /// Admits the run due at `now`, runs it and records how it ended.
    async fn night(&self, now: i64) -> (ScheduledRun, RunStatus) {
        let due = self.next(now).await.expect("a run is due");
        assert_eq!(due.plugin, Some("renoa.memory"));
        let status = self.finish(&due, now).await;
        (due, status)
    }

    async fn finish(&self, due: &ScheduledRun, now: i64) -> RunStatus {
        let finished = self
            .scheduler
            .run_for_plugin(&self.host, due, now)
            .await
            .expect("the run ends");
        let status = finished.status();
        self.scheduler
            .finish_plugin_run(finished, now)
            .await
            .expect("record the run");
        status
    }

    async fn trigger(&self) -> crate::ObservedAutomation {
        HostObserver::open(&self.data())
            .expect("observer")
            .snapshot()
            .await
            .expect("snapshot")
            .automations
            .into_iter()
            .find(|automation| automation.plugin.as_deref() == Some("renoa.memory"))
            .expect("the nightly trigger")
    }

    fn receipts(&self, run: Uuid) -> Vec<(String, Option<String>)> {
        rusqlite::Connection::open(self.database())
            .expect("catalog")
            .prepare(
                "SELECT json_extract(job_json, '$.kind'), receipt FROM host_memory_jobs
                 WHERE run_id = ?1 ORDER BY position",
            )
            .expect("receipts")
            .query_map([run.to_string()], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("receipts")
            .collect::<Result<_, _>>()
            .expect("receipts")
    }
}

async fn agent(host: &LocalHost, name: &str) -> AgentId {
    host.create_agent(
        AgentCreator::System {
            component: "memory-dream-fixture".to_owned(),
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

fn add_notes(memory: &Memory, count: u64) {
    for n in 0..count {
        memory
            .note(
                "Fixture",
                &format!("Walked the dog at dawn, day {n}"),
                at("2026-11-01T07:00"),
            )
            .expect("a note");
    }
}

fn receipt(kind: &str, receipt: &str) -> (String, Option<String>) {
    (kind.to_owned(), Some(receipt.to_owned()))
}

fn dream_log(memory: &Memory) -> String {
    fs::read_to_string(memory.dir().join(dream::LOG)).unwrap_or_default()
}

/// A valid dream answer for notes from `first`, applied outside the run.
fn mind_citing(first: u64) -> Answer {
    Answer::Reply(format!(
        "<mind>\n## Durable\n## Active\n- Walks the dog (#{first})\n## Hunches\n## Nudges\n</mind>\n"
    ))
}

#[tokio::test]
async fn a_nightly_run_compacts_then_dreams_then_writes_mind_and_moves_the_watermark() {
    let fixture = Fixture::new().await;
    let person = Uuid::new_v4();
    let memory = fixture.notebook(person, 5);

    assert!(
        fixture.next(ms(NOON)).await.is_none(),
        "nothing is due at noon"
    );
    let trigger = fixture.trigger().await;
    assert_eq!(AgentId::from_uuid(trigger.agent_id), fixture.main);
    assert_eq!(
        trigger.next_due_ms,
        ms(FIRST_NIGHT),
        "11:00 in Asia/Kolkata"
    );
    assert!(trigger.enabled && trigger.last_run.is_none());

    let (due, status) = fixture.night(ms(FIRST_NIGHT)).await;
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(
        fixture.kinds(),
        ["compaction", "compaction", "compaction", "dream"],
        "every due summary first, smallest block first, then the dream"
    );
    for call in fixture.calls() {
        assert_eq!(call["system"], MEMORY_JOB_INSTRUCTIONS, "{call}");
        assert_eq!(call["tools"], 0, "a memory turn has no tools: {call}");
        assert_eq!(call["messages"], 1, "each job sees itself alone: {call}");
        assert_eq!(call["turn_context"], false, "no message context: {call}");
    }
    let dreamed = &fixture.calls()[3];
    assert_eq!(
        dreamed["now"], "2026-11-03T11:00",
        "the frozen time, in the zone"
    );
    assert_eq!(dreamed["notes"], json!(["0", "4"]));
    assert_eq!(dreamed["profile"], true, "Dream reads USER.md");
    assert_eq!(
        dream::read_watermark(memory.dir()).expect("watermark"),
        5,
        "the dream read every note"
    );
    let mind = memory.mind().expect("MIND.md").expect("written");
    assert!(
        mind.text.contains("Walks the dog at dawn (#0)"),
        "{}",
        mind.text
    );
    assert_eq!(memory.compaction_job().expect("compaction"), None);
    assert_eq!(
        fixture.receipts(due.run.id),
        [
            receipt("compaction", "saved"),
            receipt("compaction", "saved"),
            receipt("compaction", "saved"),
            receipt("dream", "dreamed"),
        ]
    );

    let trigger = fixture.trigger().await;
    let last = trigger.last_run.expect("the last run");
    assert_eq!((last.id, last.status), (due.run.id, RunStatus::Succeeded));
    let summary = last.summary.expect("a plugin run's summary");
    assert_eq!(summary, "compaction: 3 saved; dream: 1 dreamed.");
    assert!(
        trigger.next_due_ms == ms("2026-11-04T05:30:00Z"),
        "the next night is due"
    );
    let inspected = serde_json::to_string(
        &HostObserver::open(&fixture.data())
            .expect("observer")
            .snapshot()
            .await
            .expect("snapshot"),
    )
    .expect("inspection");
    assert!(
        !inspected.contains("Walked the dog") && !inspected.contains("Walks the dog"),
        "inspection never shows a note or an answer"
    );

    let (_, quiet) = fixture.night(ms("2026-11-04T05:30:00Z")).await;
    assert_eq!(quiet, RunStatus::Succeeded);
    assert_eq!(
        fixture.calls().len(),
        4,
        "a night with nothing new asks nothing"
    );
    assert_eq!(
        fixture
            .trigger()
            .await
            .last_run
            .expect("run")
            .summary
            .as_deref(),
        Some("Nothing was due.")
    );
}

/// Crashes the first night's run at `point`, with the model taking `steps`,
/// restarts it, and returns the fixture, the run and its notebook.
async fn crash_and_restart(
    point: pass::fault::Point,
    steps: &[&str],
) -> (Fixture, ScheduledRun, Memory) {
    let fixture = Fixture::new().await;
    let memory = fixture.notebook(Uuid::new_v4(), 5);
    fixture.script(steps);
    assert!(fixture.next(ms(NOON)).await.is_none());
    let due = fixture.next(ms(FIRST_NIGHT)).await.expect("due");
    pass::fault::arm(due.run.id, point);
    assert!(
        fixture
            .scheduler
            .run_for_plugin(&fixture.host, &due, ms(FIRST_NIGHT))
            .await
            .is_err(),
        "the run stops at the injected crash"
    );
    let resumed = fixture
        .next(ms(FIRST_NIGHT) + 60_000)
        .await
        .expect("the run is still admitted");
    assert_eq!(
        resumed.run.id, due.run.id,
        "a restart hands on the same run"
    );
    assert_eq!(
        fixture.finish(&resumed, ms(FIRST_NIGHT) + 60_000).await,
        RunStatus::Succeeded
    );
    (fixture, due, memory)
}

#[tokio::test]
async fn a_crash_between_the_answer_and_its_apply_applies_it_once_after_a_restart() {
    use crate::plugins::host::memory::jobs::JobKind;
    use pass::fault::Point;

    let (fixture, due, memory) = crash_and_restart(Point::Answered(JobKind::Compaction), &[]).await;
    assert_eq!(
        fixture.kinds(),
        ["compaction", "compaction", "compaction", "dream"],
        "the answered job is replayed from the kernel, not asked again"
    );
    assert_eq!(
        fixture.receipts(due.run.id),
        [
            receipt("compaction", "saved"),
            receipt("compaction", "saved"),
            receipt("compaction", "saved"),
            receipt("dream", "dreamed"),
        ]
    );
    assert_eq!(memory.compaction_job().expect("compaction"), None);

    // Crashing after the apply and before its receipt applies the same
    // answer again, which Mnem settles without writing it twice.
    let (fixture, due, memory) = crash_and_restart(Point::Applied(JobKind::Compaction), &[]).await;
    assert_eq!(
        fixture.kinds(),
        ["compaction", "compaction", "compaction", "dream"]
    );
    assert_eq!(
        fixture.receipts(due.run.id),
        [
            receipt("compaction", "settled"),
            receipt("compaction", "saved"),
            receipt("compaction", "saved"),
            receipt("dream", "dreamed"),
        ]
    );
    assert_eq!(memory.compaction_job().expect("compaction"), None);
    assert_eq!(dream::read_watermark(memory.dir()).expect("watermark"), 5);

    // Mnem counts a decline each time it is applied, so a Dream answer whose
    // apply began is never applied again: the decline counts once.
    let steps = [
        "say:Walks the dog",
        "say:Walks the dog",
        "say:Walks the dog",
        "silent",
    ];
    let (fixture, due, memory) = crash_and_restart(Point::Applied(JobKind::Dream), &steps).await;
    assert_eq!(
        fixture.kinds(),
        ["compaction", "compaction", "compaction", "dream"]
    );
    assert_eq!(
        fixture.receipts(due.run.id).last(),
        Some(&receipt("dream", "interrupted"))
    );
    assert_eq!(
        dream_log(&memory).matches("the model declined").count(),
        1,
        "{}",
        dream_log(&memory)
    );
    assert_eq!(dream::read_watermark(memory.dir()).expect("watermark"), 0);
}

#[tokio::test]
async fn a_stale_dream_is_asked_again_in_the_same_run() {
    use crate::plugins::host::memory::jobs::JobKind;
    use pass::fault::Point;

    let fixture = Fixture::new().await;
    let memory = fixture.notebook(Uuid::new_v4(), 4);
    // Another driver's dream over the first four notes, answered later.
    let other = memory
        .dream_job(Trigger::Nightly, None, at("2026-11-02T23:00"))
        .expect("job")
        .expect("four notes make a dream due");
    add_notes(&memory, 1);
    assert!(fixture.next(ms(NOON)).await.is_none());
    let due = fixture.next(ms(FIRST_NIGHT)).await.expect("due");
    // It lands while this run's dream is frozen, and more notes follow.
    let directory = memory.dir().to_path_buf();
    pass::fault::on(due.run.id, Point::Frozen(JobKind::Dream), move || {
        let memory = Memory::open(&directory).expect("the notebook");
        memory
            .apply_dream(&other.id, mind_citing(0), at("2026-11-02T23:00"))
            .expect("the other dream lands first");
        add_notes(&memory, 4);
    });
    assert_eq!(
        fixture.finish(&due, ms(FIRST_NIGHT)).await,
        RunStatus::Succeeded
    );
    assert_eq!(
        fixture.receipts(due.run.id),
        [
            receipt("compaction", "saved"),
            receipt("compaction", "saved"),
            receipt("compaction", "saved"),
            receipt("dream", "conflict"),
            receipt("dream", "dreamed"),
        ]
    );
    let dreams: Vec<Value> = fixture
        .calls()
        .into_iter()
        .filter(|call| call["kind"] == "dream")
        .collect();
    assert_eq!(dreams.len(), 2);
    assert_eq!(dreams[0]["notes"], json!(["0", "4"]), "the stale job");
    assert_eq!(
        dreams[1]["notes"],
        json!(["4", "8"]),
        "asked again from the new watermark"
    );
    assert_eq!(dream::read_watermark(memory.dir()).expect("watermark"), 9);
}

#[tokio::test]
async fn a_busy_notebook_ends_the_run() {
    let fixture = Fixture::new().await;
    let first = fixture.notebook(Uuid::from_u128(1), 5);
    let second = fixture.notebook(Uuid::from_u128(2), 5);
    let lock = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(first.dir().join(".dream.lock"))
        .expect("the dream lock");
    lock.try_lock().expect("hold another driver's dream");

    assert!(fixture.next(ms(NOON)).await.is_none());
    let (due, status) = fixture.night(ms(FIRST_NIGHT)).await;
    assert_eq!(
        status,
        RunStatus::Succeeded,
        "a busy notebook is no failure"
    );
    assert_eq!(
        fixture.kinds(),
        ["compaction", "compaction", "compaction"],
        "the first notebook compacts, its dream is busy, and the second waits"
    );
    assert_eq!(fixture.receipts(due.run.id).len(), 3);
    assert_eq!(
        second
            .compaction_job()
            .expect("job")
            .map(|job| job.id.attempt),
        Some(0)
    );
    assert_eq!(dream::read_watermark(first.dir()).expect("watermark"), 0);
    assert!(
        fixture
            .trigger()
            .await
            .last_run
            .and_then(|run| run.summary)
            .expect("summary")
            .ends_with("Stopped: another dream held a notebook.")
    );
    drop(lock);
}

#[tokio::test]
async fn a_notebook_another_dream_takes_before_the_apply_ends_the_run() {
    use crate::plugins::host::memory::jobs::JobKind;
    use pass::fault::Point;
    use std::sync::{Arc, Mutex};

    let fixture = Fixture::new().await;
    let first = fixture.notebook(Uuid::from_u128(1), 5);
    let second = fixture.notebook(Uuid::from_u128(2), 5);
    assert!(fixture.next(ms(NOON)).await.is_none());
    let due = fixture.next(ms(FIRST_NIGHT)).await.expect("due");
    let held = Arc::new(Mutex::new(None));
    let (holder, lock) = (Arc::clone(&held), first.dir().join(".dream.lock"));
    pass::fault::on(due.run.id, Point::Answered(JobKind::Dream), move || {
        let file = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(lock)
            .expect("the dream lock");
        file.try_lock().expect("another driver's dream");
        *holder.lock().expect("holder") = Some(file);
    });
    assert_eq!(
        fixture.finish(&due, ms(FIRST_NIGHT)).await,
        RunStatus::Succeeded
    );
    assert_eq!(
        fixture.receipts(due.run.id).last(),
        Some(&receipt("dream", "busy"))
    );
    assert_eq!(
        fixture.kinds(),
        ["compaction", "compaction", "compaction", "dream"],
        "the second notebook waits for the next night"
    );
    assert_eq!(
        second
            .compaction_job()
            .expect("job")
            .map(|job| job.id.attempt),
        Some(0)
    );
    assert_eq!(dream::read_watermark(first.dir()).expect("watermark"), 0);
    drop(held);
}

#[tokio::test]
async fn a_failed_model_call_applies_nothing_and_a_decline_counts() {
    let fixture = Fixture::new().await;
    let memory = fixture.notebook(Uuid::new_v4(), 5);
    fixture.script(&[
        "say:Walks the dog",
        "say:Walks the dog",
        "say:Walks the dog",
        "fail",
    ]);
    assert!(fixture.next(ms(NOON)).await.is_none());
    let (due, status) = fixture.night(ms(FIRST_NIGHT)).await;
    assert_eq!(status, RunStatus::Failed);
    assert_eq!(
        fixture.receipts(due.run.id).last(),
        Some(&receipt("dream", "unanswered"))
    );
    assert_eq!(dream::read_watermark(memory.dir()).expect("watermark"), 0);
    assert!(
        !dream_log(&memory).contains("declined"),
        "an unanswered dream is never applied, so it does not count toward giving up"
    );

    fixture.script(&["silent"]);
    let (due, status) = fixture.night(ms("2026-11-04T05:30:00Z")).await;
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(fixture.receipts(due.run.id), [receipt("dream", "declined")]);
    assert!(
        dream_log(&memory).contains("the model declined notes #0 to #5"),
        "{}",
        dream_log(&memory)
    );
    assert_eq!(dream::read_watermark(memory.dir()).expect("watermark"), 0);
}

#[tokio::test]
async fn an_invalid_answer_stops_its_compaction_and_the_run_goes_on() {
    let fixture = Fixture::new().await;
    let first = fixture.notebook(Uuid::from_u128(1), 5);
    let second = fixture.notebook(Uuid::from_u128(2), 5);
    fixture.script(&["echo"]);
    assert!(fixture.next(ms(NOON)).await.is_none());
    let (due, status) = fixture.night(ms(FIRST_NIGHT)).await;
    assert_eq!(status, RunStatus::Failed);
    assert_eq!(
        fixture.kinds(),
        [
            "compaction",
            "dream",
            "compaction",
            "compaction",
            "compaction",
            "dream"
        ],
        "the first notebook's compaction stops, its Dream still runs, and the second is untouched"
    );
    assert_eq!(
        fixture.receipts(due.run.id),
        [
            receipt("compaction", "invalid"),
            receipt("dream", "dreamed"),
            receipt("compaction", "saved"),
            receipt("compaction", "saved"),
            receipt("compaction", "saved"),
            receipt("dream", "dreamed"),
        ]
    );
    assert_eq!(
        first
            .compaction_job()
            .expect("job")
            .map(|job| job.id.attempt),
        Some(0)
    );
    assert_eq!(dream::read_watermark(first.dir()).expect("watermark"), 5);
    assert_eq!(dream::read_watermark(second.dir()).expect("watermark"), 5);
    assert!(
        fixture
            .trigger()
            .await
            .last_run
            .and_then(|run| run.summary)
            .expect("summary")
            .ends_with("Stopped: Mnem refused a compaction answer as invalid")
    );
}

#[tokio::test]
async fn a_notebook_that_cannot_be_opened_is_skipped_whole() {
    let fixture = Fixture::new().await;
    let first = fixture.notebook(Uuid::from_u128(1), 5);
    let second = fixture.notebook(Uuid::from_u128(2), 5);
    fs::remove_dir_all(first.dir()).expect("remove the notebook");
    fs::write(first.dir(), "not a notebook").expect("a file in its place");
    assert!(fixture.next(ms(NOON)).await.is_none());
    let (due, status) = fixture.night(ms(FIRST_NIGHT)).await;
    assert_eq!(status, RunStatus::Failed);
    assert_eq!(
        fixture.receipts(due.run.id).len(),
        4,
        "the second notebook goes on"
    );
    assert_eq!(dream::read_watermark(second.dir()).expect("watermark"), 5);
    let summary = fixture
        .trigger()
        .await
        .last_run
        .and_then(|run| run.summary)
        .expect("summary");
    assert_eq!(
        summary.matches("not a plain directory").count(),
        1,
        "its Dream is not tried: {summary}"
    );
}

#[tokio::test]
async fn no_agent_lists_reads_changes_or_runs_the_trigger() {
    let fixture = Fixture::new().await;
    fixture.notebook(Uuid::new_v4(), 5);
    assert!(fixture.next(ms(NOON)).await.is_none());
    let trigger = fixture.trigger().await;
    let (main, host) = (fixture.main, &fixture.host);
    assert!(
        host.list_automations(main, main, None)
            .await
            .expect("list")
            .is_empty(),
        "automation_manage list does not show it"
    );
    assert!(host.automation(main, trigger.id).await.is_err());
    for mutation in [
        AutomationMutation::RunNow { id: trigger.id },
        AutomationMutation::SetEnabled {
            id: trigger.id,
            expected_revision: trigger.revision,
            enabled: false,
        },
        AutomationMutation::Delete {
            id: trigger.id,
            expected_revision: trigger.revision,
        },
    ] {
        assert!(
            host.manage_automation(
                main,
                Uuid::new_v4(),
                mutation,
                ms(NOON),
                CancellationToken::new()
            )
            .await
            .is_err()
        );
    }
    let owner = Uuid::new_v4();
    let control = HostAutomationControl::open(
        &fixture.data(),
        host.host_id().await.expect("host id"),
        owner,
    )
    .expect("owner control");
    assert!(
        control
            .set_enabled(
                owner,
                trigger.id,
                crate::AutomationEnablement {
                    operation_id: Uuid::new_v4(),
                    expected_revision: trigger.revision,
                    enabled: false,
                },
                ms(NOON),
            )
            .await
            .is_err(),
        "the owner turns the plugin off instead"
    );

    let (due, _) = fixture.night(ms(FIRST_NIGHT)).await;
    assert!(
        host.automation_results(main, main, None)
            .await
            .expect("results")
            .is_empty()
    );
    assert!(host.automation_result(main, due.run.id).await.is_err());
    assert!(
        host.completed_automation_runs(0)
            .await
            .expect("completed")
            .is_empty()
    );
    assert_eq!(
        fixture.trigger().await.revision,
        trigger.revision,
        "unchanged"
    );
}

#[tokio::test]
async fn a_job_a_crash_left_waits_while_the_plugin_is_off() {
    use crate::plugins::host::memory::jobs::JobKind;
    use pass::fault::Point;

    let fixture = Fixture::new().await;
    fixture.notebook(Uuid::new_v4(), 5);
    assert!(fixture.next(ms(NOON)).await.is_none());
    let due = fixture.next(ms(FIRST_NIGHT)).await.expect("due");
    pass::fault::arm(due.run.id, Point::Frozen(JobKind::Compaction));
    assert!(
        fixture
            .scheduler
            .run_for_plugin(&fixture.host, &due, ms(FIRST_NIGHT))
            .await
            .is_err()
    );
    state::change(
        fixture.database(),
        fixture.main,
        HostPluginId::Memory,
        false,
        "off",
    )
    .expect("off");
    let resumed = fixture.next(ms(FIRST_NIGHT)).await.expect("resumed");
    fixture.finish(&resumed, ms(FIRST_NIGHT)).await;
    assert!(
        fixture.calls().is_empty(),
        "nothing is asked while it is off"
    );
    assert_eq!(
        fixture.receipts(due.run.id),
        [("compaction".to_owned(), None)],
        "the frozen job waits for the plugin"
    );
}

#[tokio::test]
async fn there_is_no_run_with_the_plugin_off_or_without_a_main_agent() {
    let fixture = Fixture::new().await;
    fixture.notebook(Uuid::new_v4(), 5);
    let database = fixture.database().to_path_buf();
    assert!(fixture.next(ms(NOON)).await.is_none());

    // Turned off after its run was admitted: the run does nothing.
    let due = fixture.next(ms(FIRST_NIGHT)).await.expect("admitted");
    state::change(&database, fixture.main, HostPluginId::Memory, false, "off").expect("off");
    assert_eq!(
        fixture.finish(&due, ms(FIRST_NIGHT)).await,
        RunStatus::Skipped
    );
    assert!(fixture.calls().is_empty(), "no job ran");
    assert!(fixture.next(ms("2026-11-04T05:30:00Z")).await.is_none());
    let off = fixture.trigger().await;
    assert!(!off.enabled, "the trigger is disabled with its plugin");

    // Back on: due again from now, as an edit would be.
    state::change(&database, fixture.main, HostPluginId::Memory, true, "on").expect("on");
    assert!(fixture.next(ms("2026-11-04T12:00:00Z")).await.is_none());
    let on = fixture.trigger().await;
    assert!(on.enabled && on.revision > off.revision);
    assert_eq!(on.next_due_ms, ms("2026-11-05T05:30:00Z"));

    // A new main agent takes the trigger; the old one's stays, disabled.
    let other = agent(&fixture.host, "Scout").await;
    let connection = rusqlite::Connection::open(&database).expect("catalog");
    connection
        .execute(
            "UPDATE host_main_agent SET agent_id = ?1, revision = revision + 1",
            [other.to_string()],
        )
        .expect("a new main agent");
    assert!(fixture.next(ms("2026-11-04T12:00:00Z")).await.is_none());
    let observed = HostObserver::open(&fixture.data())
        .expect("observer")
        .snapshot()
        .await
        .expect("snapshot")
        .automations;
    let enabled: Vec<AgentId> = observed
        .iter()
        .filter(|automation| automation.plugin.is_some() && automation.enabled)
        .map(|automation| AgentId::from_uuid(automation.agent_id))
        .collect();
    assert_eq!(enabled, [other]);
    assert_eq!(observed.iter().filter(|a| a.plugin.is_some()).count(), 2);

    // No main agent: nothing runs.
    connection
        .execute("DELETE FROM host_main_agent", [])
        .expect("no main agent");
    assert!(fixture.next(ms("2026-11-06T05:30:00Z")).await.is_none());
    assert!(fixture.calls().is_empty());
}

#[tokio::test]
async fn a_missed_night_coalesces_into_one_run_or_is_skipped_when_too_late() {
    let fixture = Fixture::new().await;
    fixture.notebook(Uuid::new_v4(), 5);
    assert!(fixture.next(ms(NOON)).await.is_none());

    // Down from before 11:00 until 17:00: one run, six hours late.
    let late = ms(FIRST_NIGHT) + 6 * 3_600_000;
    let (due, status) = fixture.night(late).await;
    assert_eq!(
        (due.run.due_ms, status),
        (ms(FIRST_NIGHT), RunStatus::Succeeded)
    );
    assert!(
        fixture.next(late).await.is_none(),
        "one run for the missed night"
    );

    // Down for three nights: one occurrence, skipped as too late, and the
    // next night is due; the watermark carries the notes over.
    let calls = fixture.calls().len();
    let back = ms("2026-11-06T18:00:00Z");
    let skipped = fixture.next(back).await.expect("one coalesced occurrence");
    assert_eq!(
        skipped.run.result.map(|result| result.status),
        Some(RunStatus::Skipped)
    );
    assert!(
        fixture.next(back).await.is_none(),
        "missed nights coalesce into it"
    );
    assert_eq!(
        fixture.trigger().await.next_due_ms,
        ms("2026-11-07T05:30:00Z")
    );
    assert_eq!(fixture.calls().len(), calls, "a skipped run asks nothing");
}

#[tokio::test]
async fn schema_44_adds_the_trigger_and_job_tables_to_a_schema_43_catalog() {
    let fixture = Fixture::new().await;
    assert!(fixture.next(ms(NOON)).await.is_none());
    let database = fixture.database().to_path_buf();
    let connection = rusqlite::Connection::open(&database).expect("catalog");
    crate::host::catalog::restore_schema_43(&connection);
    connection
        .execute_batch(
            "UPDATE host_metadata SET schema_version = 43 WHERE singleton = 1;
             PRAGMA user_version = 43;",
        )
        .expect("store schema 43");
    drop(connection);
    crate::host::catalog::initialize(&database).expect("upgrade schema 43");
    crate::host::catalog::initialize(&database).expect("reopen the upgraded catalog");
    assert!(fixture.next(ms(NOON)).await.is_none());
    assert_eq!(fixture.trigger().await.next_due_ms, ms(FIRST_NIGHT));
    assert_eq!(
        fixture.night(ms(FIRST_NIGHT)).await.1,
        RunStatus::Succeeded,
        "the job and session tables are in place"
    );
}

#[tokio::test]
async fn a_trigger_that_cannot_converge_never_stops_the_scheduler() {
    let fixture = Fixture::new().await;
    assert!(fixture.next(ms(NOON)).await.is_none());
    let trigger = fixture.trigger().await;
    rusqlite::Connection::open(fixture.database())
        .expect("catalog")
        .execute(
            "UPDATE host_agent_plugin_settings SET settings_json = '{\"timezone\":\"Nowhere/Atlantis\"}'
             WHERE plugin_id = 'renoa.time'",
            [],
        )
        .expect("a zone the trigger cannot be built for");
    assert!(
        fixture.next(ms(NOON) + 60_000).await.is_none(),
        "the scheduler still answers"
    );
    let kept = fixture.trigger().await;
    assert_eq!(
        (kept.revision, kept.next_due_ms),
        (trigger.revision, trigger.next_due_ms),
        "the stored trigger is kept as it was"
    );
}

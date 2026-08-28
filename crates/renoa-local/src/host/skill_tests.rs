use std::{fs, path::Path, sync::Arc};

use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock, Message};
use tempfile::tempdir;
use uuid::Uuid;

use super::{HostInitialization, LocalHost};
use crate::{LocalTurnOutcome, ModelProvider};

const PROJECT_SKILL: &str = "renoa-test-project-workflow";
const HOT_SKILL: &str = "renoa-test-hot-workflow";
const PROJECT_MARKER: &str = "PROJECT_SKILL_EXACT_INSTRUCTIONS";
const HOT_MARKER: &str = "HOT_SKILL_EXACT_INSTRUCTIONS";

#[tokio::test]
async fn skills_hot_load_and_survive_compaction_and_host_restart() {
    let directory = tempdir().expect("temporary Host directory");
    let data = directory.path().join("data");
    let workspace = directory.path().join("workspace");
    let global = directory.path().join("global-skills");
    let bridge = directory.path().join("model-bridge.mjs");
    let credentials = directory.path().join("credentials.sqlite3");
    fs::create_dir_all(&workspace).expect("create workspace");
    fs::create_dir_all(&global).expect("create empty global skill source");
    fs::write(&bridge, MODEL_BRIDGE).expect("write deterministic model bridge");
    fs::write(&credentials, "").expect("write credential placeholder");
    write_skill(
        &workspace,
        PROJECT_SKILL,
        "Project workflow for the integration proof.",
        PROJECT_MARKER,
    );
    let host = local_host(&data, &bridge, &credentials, &global);
    let session = host
        .create_alpha_session(&workspace)
        .await
        .expect("create Alpha session");
    let session_id = session.id();

    assert_eq!(
        session
            .execute_turn(
                Uuid::new_v4(),
                vec![ContentBlock::text("Activate the project workflow.")],
                Arc::new(NoopEvents),
            )
            .await
            .expect("activate project skill through Alpha"),
        LocalTurnOutcome::Completed {
            output: "Project skill activated.".to_owned(),
            stop_reason: renoa_agent::StopReason::Stop,
        }
    );

    write_skill(
        &workspace,
        HOT_SKILL,
        "Workflow added while the session is live.",
        HOT_MARKER,
    );
    assert_eq!(
        session
            .execute_turn(
                Uuid::new_v4(),
                vec![ContentBlock::text("Activate the hot-added workflow.")],
                Arc::new(NoopEvents),
            )
            .await
            .expect("discover a skill without restarting Host or session"),
        LocalTurnOutcome::Completed {
            output: "Hot skill activated.".to_owned(),
            stop_reason: renoa_agent::StopReason::Stop,
        }
    );
    let compacted = session
        .execute_compaction(Uuid::new_v4(), Arc::new(NoopEvents))
        .await
        .expect("compact a session with active skills");
    assert!(matches!(compacted, LocalTurnOutcome::Compacted { .. }));
    assert_durable_full_results(&session.history().expect("load durable pre-restart history"));

    drop(session);
    drop(host);
    let reopened = local_host(&data, &bridge, &credentials, &global);
    let restored = reopened
        .load_alpha_session(session_id, &workspace)
        .await
        .expect("restore exact Alpha session");
    assert_eq!(
        restored
            .execute_turn(
                Uuid::new_v4(),
                vec![ContentBlock::text("Confirm restored workflows.")],
                Arc::new(NoopEvents),
            )
            .await
            .expect("run with exact skills after compaction and restart"),
        LocalTurnOutcome::Completed {
            output: "Both skills restored.".to_owned(),
            stop_reason: renoa_agent::StopReason::Stop,
        }
    );
    assert_durable_full_results(&restored.history().expect("load durable restored history"));
}

fn local_host(data: &Path, bridge: &Path, credentials: &Path, global: &Path) -> LocalHost {
    LocalHost::assemble(HostInitialization {
        data_directory: data.to_path_buf(),
        bridge: bridge.to_path_buf(),
        providers: vec![ModelProvider::Xai],
        initial_provider: ModelProvider::Xai,
        initial_model: "fixture-model".to_owned(),
        credential_store: credentials.to_path_buf(),
        mcp_adapter: None,
        integration_catalog_adapter: None,
        global_skill_source: Some(global.to_path_buf()),
    })
    .expect("assemble local Host with isolated skill sources")
}

fn write_skill(workspace: &Path, name: &str, description: &str, body: &str) {
    let directory = workspace.join(".agents/skills").join(name);
    fs::create_dir_all(&directory).expect("create project skill");
    fs::write(
        directory.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n{body}\n"),
    )
    .expect("write project skill");
}

fn assert_durable_full_results(history: &[crate::LocalHistoryEntry]) {
    let loaded = history
        .iter()
        .filter_map(|entry| match &entry.message {
            Message::Tool { result } if result.name == "skill_load" => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(loaded.len(), 2);
    assert!(result_contains(loaded[0], PROJECT_MARKER));
    assert!(result_contains(loaded[1], HOT_MARKER));
}

fn result_contains(result: &renoa_agent::ToolResult, marker: &str) -> bool {
    result
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::Text { text } if text.contains(marker)))
}

struct NoopEvents;

impl AgentEventSink for NoopEvents {
    fn emit(&self, _event: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(std::future::ready(()))
    }
}

const MODEL_BRIDGE: &str = r#"
import { createHash as hash } from "node:crypto";
let input = "";
for await (const chunk of process.stdin) input += chunk;
const action = process.env.RENOA_MODEL_ACTION;
const modelSpec = JSON.stringify({ id: "fixture-model" });
if (action === "catalog") {
  process.stdout.write(JSON.stringify({ ok: true, response: { models: [{
    id: "fixture-model",
    name: "Fixture Model",
    reasoning_levels: ["high"],
    context_window_tokens: 500000,
    model_spec: { id: "fixture-model" }
  }] } }));
  process.exit(0);
}
if (action === "describe") {
  process.stdout.write(JSON.stringify({ ok: true, response: {
    context_window_tokens: 500000,
    max_output_tokens: 32768,
    model_spec: modelSpec,
    model_binding_id: hash("sha256").update(modelSpec).digest("hex"),
    reasoning_level: "high"
  } }));
  process.exit(0);
}
if (action !== "stream") process.exit(2);
const request = JSON.parse(input);
const fail = message => { process.stderr.write(message); process.exit(3); };
const complete = (content, stopReason = "stop") => {
  process.stdout.write(JSON.stringify({
    event: "completed",
    response: {
      content,
      stop_reason: stopReason,
      usage: { input: 10, output: 2, cache_read: 0, cache_write: 0 },
      metadata: { api: "test", provider: "xai", model: "fixture-model" }
    }
  }) + "\n");
};
if (request.system_prompt.startsWith("You create durable context checkpoints")) {
  if (request.tools.length !== 0) fail("compaction advertised tools");
  complete([{ type: "text", text: `## Goal and user intent
Keep testing durable skills.
## Hard constraints and preferences
Keep exact skill revisions.
## Completed work
Activated two workflows.
## Current state and blockers
No blockers.
## Decisions and rationale
Skills are Host-owned.
## Exact working facts
Two skills are active.
## Next action and unresolved questions
Confirm restoration.` }]);
  process.exit(0);
}
const expectedTools = [
  "read_file", "edit_file", "write_file", "bash", "grep", "find",
  "tool_search", "tool_load", "tool_execute", "extension_manage",
  "skill_search", "skill_load"
];
if (request.tools.map(tool => tool.name).join(",") !== expectedTools.join(",")) {
  fail("unexpected model-visible tool set");
}
const skillLoad = request.tools.find(tool => tool.name === "skill_load");
if (
  !skillLoad ||
  Object.keys(skillLoad.input_schema.properties).join(",") !== "name" ||
  skillLoad.input_schema.required.join(",") !== "name" ||
  skillLoad.input_schema.additionalProperties !== false
) fail("skill_load does not advertise its name-only input");
const promptIndex = request.messages.findLastIndex(message => message.role === "user");
const prompt = request.messages[promptIndex].content[0].text;
const results = request.messages.slice(promptIndex + 1).filter(message => message.role === "tool");
const priorMessages = JSON.stringify(request.messages.slice(0, promptIndex));
const search = (query, id) => complete([{
  type: "tool_call", id, name: "skill_search", arguments: { query }
}], "tool_use");
const load = (result, expectedName, id) => {
  const found = JSON.parse(result.result.content[0].text);
  if (!Array.isArray(found)) fail("skill search result is not an array");
  const match = found.find(entry => entry.name === expectedName);
  if (!match || Object.keys(match).sort().join(",") !== "description,name") {
    fail("skill search exposed more than name and description");
  }
  complete([{ type: "tool_call", id, name: "skill_load", arguments: {
    name: match.name
  } }], "tool_use");
};
if (prompt === "Activate the project workflow.") {
  if (results.length === 0) search("renoa-test-project-workflow", "project-search");
  else if (results.length === 1) load(results[0], "renoa-test-project-workflow", "project-load");
  else if (
    results.length === 2 &&
    results[1].result.name === "skill_load" &&
    results[1].result.content[0].text.includes("PROJECT_SKILL_EXACT_INSTRUCTIONS")
  ) complete([{ type: "text", text: "Project skill activated." }]);
  else fail("invalid project skill flow");
} else if (prompt === "Activate the hot-added workflow.") {
  if (!request.system_prompt.includes("PROJECT_SKILL_EXACT_INSTRUCTIONS")) {
    fail("active project skill missing from system prompt");
  }
  if (priorMessages.includes("PROJECT_SKILL_EXACT_INSTRUCTIONS")) {
    fail("active project skill duplicated in historical messages");
  }
  if (!priorMessages.includes("remains active; its exact instructions are reattached above")) {
    fail("historical skill result was not projected to a receipt");
  }
  if (results.length === 0) search("renoa-test-hot-workflow", "hot-search");
  else if (results.length === 1) load(results[0], "renoa-test-hot-workflow", "hot-load");
  else if (
    results.length === 2 &&
    results[1].result.name === "skill_load" &&
    results[1].result.content[0].text.includes("HOT_SKILL_EXACT_INSTRUCTIONS")
  ) complete([{ type: "text", text: "Hot skill activated." }]);
  else fail("invalid hot skill flow");
} else if (prompt === "Confirm restored workflows.") {
  if (
    !request.system_prompt.includes("PROJECT_SKILL_EXACT_INSTRUCTIONS") ||
    !request.system_prompt.includes("HOT_SKILL_EXACT_INSTRUCTIONS")
  ) fail("durable skills were not restored exactly");
  if (priorMessages.includes("PROJECT_SKILL_EXACT_INSTRUCTIONS") ||
      priorMessages.includes("HOT_SKILL_EXACT_INSTRUCTIONS")) {
    fail("restored skill instructions were duplicated in transcript history");
  }
  complete([{ type: "text", text: "Both skills restored." }]);
} else {
  fail("unexpected prompt");
}
"#;

use std::path::Path;

use tokio::time::{Duration, sleep, timeout};

pub(crate) async fn wait_for_path(path: &Path) {
    timeout(Duration::from_secs(5), async {
        while !path.exists() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("timed out waiting for model marker");
}

/// A tool turn whose final model call waits for release, so its tool events
/// can only reach a surface while the turn runs.
const HELD_TOOL_TURN: &str = r#"} else if (prompt === "Read proof, then wait." && toolResults.length === 0) {
  content = [
    { type: "text", text: "Reading the proof first." },
    { type: "tool_call", id: "read-held", name: "read_file", arguments: { path: "proof.txt" } }
  ];
  stopReason = "tool_use";
} else if (prompt === "Read proof, then wait.") {
  writeFileSync(join(workspace, "model-started"), "started");
  while (!existsSync(join(workspace, "model-release"))) {
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  content = [{ type: "text", text: "The held proof was read." }];
}"#;

/// Creates a paused automation through the Host plugin, discovering its
/// schema first as a model must, and answers the automation's standing
/// prompts: one that succeeds, one whose model fails, and one whose tool call
/// fails before it answers.
/// "Schedule for <agent>: <standing prompt>" names the automation's agent.
const AUTOMATION_TURNS: &str = r#"} else if (prompt.startsWith("Schedule for ")) {
  const [, agent, standing] = prompt.match(/^Schedule for ([^:]+): (.*)$/);
  const turn = request.messages.slice(request.messages.findLastIndex(message => message.role === "user") + 1)
    .filter(message => message.role === "tool");
  const last = turn.at(-1);
  if (last?.result.is_error) throw Error(JSON.stringify(last.result));
  const found = last && JSON.parse(last.result.content[0].text);
  stopReason = "tool_use";
  if (turn.length === 0) {
    content = [{ type: "tool_call", id: "find-automations", name: "plugin_search", arguments: { query: "automation_manage" } }];
  } else if (turn.length === 1) {
    const match = found.tool_matches.find(tool => tool.name === "automation_manage");
    content = [{ type: "tool_call", id: "describe-automations", name: "plugin_search", arguments: { reference: match.reference } }];
  } else if (turn.length === 2) {
    content = [{ type: "tool_call", id: "create-automation", name: "tool_execute", arguments: {
      reference: found.reference,
      arguments: { action: "create", spec: {
        agent_id: agent, name: "Digest", prompt: standing,
        schedule: { kind: "interval", hours: 24 }, enabled: false
      } }
    } }];
  } else {
    content = [{ type: "text", text: "Scheduled." }];
    stopReason = "stop";
  }
} else if (prompt === "Write the digest.") {
  content = [{ type: "text", text: "Digest written." }];
} else if (prompt === "Fail the digest.") {
  process.stderr.write("the model is unavailable");
  process.exit(4);
} else if (prompt === "Read the missing notes." && toolResults.length === 0) {
  content = [{ type: "tool_call", id: "read-missing", name: "read_file", arguments: { path: "missing-notes.txt" } }];
  stopReason = "tool_use";
} else if (prompt === "Read the missing notes.") {
  content = [{ type: "text", text: "The notes are missing." }];"#;

pub(super) fn bridge_script(workspace: &Path) -> String {
    let workspace = serde_json::to_string(&workspace.to_string_lossy()).expect("encode workspace");
    format!(
        r#"
import {{ existsSync, readFileSync, writeFileSync }} from "node:fs";
import {{ join }} from "node:path";
import {{ createHash }} from "node:crypto";
let input = "";
for await (const chunk of process.stdin) input += chunk;
const action = process.env.RENOA_MODEL_ACTION;
const workspace = {workspace};
if (action === "catalog") {{
  process.stdout.write(JSON.stringify({{ ok: true, response: {{ models: [{{
    id: "fixture-model",
    name: "Fixture Model",
    reasoning_levels: ["high"],
    context_window_tokens: 100000,
    model_spec: {{ id: "fixture-model" }}
  }}] }} }}));
  process.exit(0);
}}
if (action === "describe") {{
  const modelSpec = JSON.stringify({{ id: "fixture-model" }});
  process.stdout.write(JSON.stringify({{ ok: true, response: {{
    context_window_tokens: 100000,
    max_output_tokens: 8192,
    model_spec: modelSpec,
    model_binding_id: createHash("sha256").update(modelSpec).digest("hex"),
    reasoning_level: "high"
  }} }}));
  process.exit(0);
}}
if (action !== "stream") process.exit(2);
const request = JSON.parse(input);
const user = request.messages.findLast(message => message.role === "user");
const prompt = user?.content.find(block => block.type === "text")?.text ?? "";
const toolResults = request.messages.filter(message => message.role === "tool");
let content;
let stopReason = "stop";
if (prompt === "Read proof." && toolResults.length === 0) {{
  content = [{{
    type: "tool_call", id: "read-proof", name: "read_file",
    arguments: {{ path: "proof.txt" }}
  }}];
  stopReason = "tool_use";
}} else if (prompt === "Read proof.") {{
  content = [{{ type: "text", text: "The durable proof was read." }}];
{HELD_TOOL_TURN} else if (prompt === "Hold through reconnect.") {{
  writeFileSync(join(workspace, "model-started"), "started");
  while (!existsSync(join(workspace, "model-release"))) {{
    await new Promise(resolve => setTimeout(resolve, 10));
  }}
  content = [{{ type: "text", text: "Finished after reconnect." }}];
}} else if (prompt === "Crash model.") {{
  const attemptsPath = join(workspace, "model-attempts");
  const attempts = existsSync(attemptsPath)
    ? Number.parseInt(readFileSync(attemptsPath, "utf8"), 10)
    : 0;
  writeFileSync(attemptsPath, String(attempts + 1));
  if (attempts === 0) {{
    writeFileSync(join(workspace, "model-started"), "started");
    while (true) await new Promise(resolve => setTimeout(resolve, 10));
  }}
  content = [{{ type: "text", text: "Recovered the same Host turn." }}];
}} else if (prompt === "Parallel one." || prompt === "Parallel two.") {{
  const suffix = prompt === "Parallel one." ? "one" : "two";
  writeFileSync(join(workspace, `model-started-${{suffix}}`), "started");
  while (!existsSync(join(workspace, `model-release-${{suffix}}`))) {{
    await new Promise(resolve => setTimeout(resolve, 10));
  }}
  content = [{{ type: "text", text: `Finished parallel ${{suffix}}.` }}];
}} else if (prompt === "Which profile do you see?") {{
  const seen = JSON.stringify(request).match(/PROFILE_[A-Z]+/g) ?? ["none"];
  content = [{{ type: "text", text: seen.join(",") }}];
{AUTOMATION_TURNS}}} else if (prompt === "First.") {{
  content = [{{ type: "text", text: "First response." }}];
}} else if (prompt === "Second.") {{
  content = [{{ type: "text", text: "Second response." }}];
}} else {{
  process.stderr.write(`unexpected fixture prompt: ${{prompt}}`);
  process.exit(3);
}}
process.stdout.write(JSON.stringify({{
  ok: true,
  response: {{
    content,
    stop_reason: stopReason,
    usage: {{ input: 8, output: 4, cache_read: 0, cache_write: 0 }},
    metadata: {{ api: "test", provider: "xai", model: "fixture-model" }}
  }}
}}));
"#
    )
}

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

/// A turn that streams its answer: private reasoning, then visible text in two
/// deltas further apart than the node's flush interval, then it waits for
/// release before it completes with the whole text.
const STREAMED_TURN: &str = r#" else if (prompt === "Stream an answer.") {
  const record = value => process.stdout.write(JSON.stringify(value) + "\n");
  const delta = (type, text) => record({ event: "content_delta", content_index: 0, delta: { type, text } });
  delta("reasoning", "private deliberation");
  delta("text", "Hello ");
  await new Promise(resolve => setTimeout(resolve, 400));
  delta("text", "world");
  while (!existsSync(join(workspace, "model-release"))) {
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  content = [{ type: "text", text: "Hello world" }];
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
        schedule: { kind: "cron", expression: "0 0 * * *", timezone: "UTC" }, enabled: false
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

/// Turns that report what the Host showed the model for one speaker.
const SPEAKER_TURNS: &str = r#"} else if (prompt === "Read the proof for me." && toolResults.length === 0) {
  content = [{ type: "tool_call", id: "read-for", name: "read_file", arguments: { path: "proof.txt" } }];
  stopReason = "tool_use";
} else if (prompt === "Read the proof for me.") {
  const result = toolResults.at(-1).result;
  const profile = JSON.stringify(request).match(/PROFILE_[A-Z]+/g) ?? ["none"];
  content = [{ type: "text", text: `${result.is_error ? "refused" : "read"}; profile ${profile.join(",")}` }];
} else if (prompt === "Where am I?") {
  const seen = user.content.map(block => block.text ?? "").join("\n")
    .match(/<context source="surface">\n([\s\S]*?)\n<\/context>/);
  content = [{ type: "text", text: seen ? seen[1] : "nowhere" }];
} else if (prompt === "Which profile do you see?") {
  const seen = JSON.stringify(request).match(/PROFILE_[A-Z]+/g) ?? ["none"];
  content = [{ type: "text", text: seen.join(",") }];
"#;

/// What the fixture model answers when asked for a context summary: a valid
/// checkpoint, an invalid one, or nothing until its process is replaced.
const SUMMARY_TURN: &str = r###"if (summarizing) {
  const attemptsPath = join(workspace, "summary-attempts");
  const attempts = existsSync(attemptsPath)
    ? Number.parseInt(readFileSync(attemptsPath, "utf8"), 10)
    : 0;
  writeFileSync(attemptsPath, String(attempts + 1));
  writeFileSync(join(workspace, "summary-request.json"), input);
  if (input.includes("Break the summary.")) {
    content = [{ type: "text", text: "This is not a checkpoint." }];
  } else if (input.includes("Hold the summary.") && attempts === 0) {
    writeFileSync(join(workspace, "model-started"), "started");
    while (true) await new Promise(resolve => setTimeout(resolve, 10));
  } else {
    // Streamed as a reply would be, so a surface would see it if a summary's
    // message events reached the node.
    process.stdout.write(JSON.stringify({ event: "content_delta", content_index: 0,
      delta: { type: "text", text: "## Goal and user intent" } }) + "\n");
    content = [{ type: "text", text: [
      "## Goal and user intent\nTalk with the owner.",
      "## Hard constraints and preferences\nNone yet.",
      "## Completed work\nAnswered earlier questions.",
      "## Current state and blockers\nNo blocker.",
      "## Decisions and rationale\nSummarize durably.",
      "## Exact working facts\nThe proof file holds durable proof.",
      "## Next action and unresolved questions\nAwait the next message.",
    ].join("\n") }];
  }
}"###;

/// Conversations the owner's `/compact` summarizes, a guest's `/compact`, which
/// reaches the model as ordinary text, and two long messages that together
/// overflow the fixture model's window.
const COMPACTION_TURNS: &str = r#"} else if (prompt === "Break the summary." || prompt === "Hold the summary.") {
  content = [{ type: "text", text: "Noted." }];
} else if (prompt === "/compact") {
  content = [{ type: "text", text: "A guest asked: /compact" }];
} else if (prompt.startsWith("Long message one.")) {
  content = [{ type: "text", text: "Long answer one." }];
} else if (prompt.startsWith("Long message two.")) {
  process.stdout.write(JSON.stringify({ event: "content_delta", content_index: 0,
    delta: { type: "text", text: "Long answer two." } }) + "\n");
  content = [{ type: "text", text: "Long answer two." }];
"#;

/// Refuses a request a provider would reject: a tool result without its call,
/// or a call left without its result before the next message.
const PAIRING_CHECK: &str = r#"const pending = new Set();
for (const message of request.messages) {
  if (message.role === "assistant") {
    if (pending.size > 0) throw Error("an assistant message came before every tool result");
    for (const block of message.content) if (block.type === "tool_call") pending.add(block.id);
  } else if (message.role === "tool") {
    if (!pending.delete(message.result.call_id)) throw Error(`tool result ${message.result.call_id} has no call`);
  } else if (pending.size > 0) {
    throw Error("a user message came before every tool result");
  }
}
if (pending.size > 0) throw Error("tool calls were left without results");"#;

/// "Ask <agent>: <question>" messages the named agent through the Host plugin,
/// discovering its schema first as a model must. The question's recipient
/// answers "Count the files." with what its turn context says about the
/// sender, and the asker answers that reply the same way.
const AGENT_TURNS: &str = r#"} else if (prompt.startsWith("Ask ")) {
  const [, agent, question] = prompt.match(/^Ask ([^:]+): (.*)$/);
  const turn = request.messages.slice(request.messages.findLastIndex(message => message.role === "user") + 1)
    .filter(message => message.role === "tool");
  const last = turn.at(-1);
  // The discovery results are JSON; the message's own result is plain text.
  const found = () => JSON.parse(last.result.content[0].text);
  stopReason = "tool_use";
  if (turn.length === 0) {
    content = [{ type: "tool_call", id: "find-message", name: "plugin_search", arguments: { query: "message_agent" } }];
  } else if (turn.length === 1) {
    const match = found().tool_matches.find(tool => tool.name === "message_agent");
    content = [{ type: "tool_call", id: "describe-message", name: "plugin_search", arguments: { reference: match.reference } }];
  } else if (turn.length === 2) {
    content = [{ type: "tool_call", id: "send-message", name: "tool_execute", arguments: {
      reference: found().reference,
      arguments: { to: agent, text: question }
    } }];
  } else {
    content = [{ type: "text", text: "Asked." }];
    stopReason = "stop";
  }
} else if (prompt === "Count the files." || prompt.startsWith("Count: 3.")) {
  const seen = user.content.map(block => block.text ?? "").join("\n")
    .match(/<context source="surface">\n([\s\S]*?)\n<\/context>/);
  const context = seen ? seen[1] : "nowhere";
  content = [{ type: "text", text: prompt === "Count the files." ? `Count: 3. ${context}` : `Thanks. ${context}` }];"#;

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
writeFileSync(join(workspace, "last-request.json"), input);
{PAIRING_CHECK}
const summarizing = request.system_prompt.startsWith("You create durable context checkpoints");
const user = request.messages.findLast(message => message.role === "user");
// An automation run's task follows its one context line.
const prompt = (user?.content.find(block => block.type === "text")?.text ?? "")
  .replace(/^\((Scheduled|Requested) run [^\n]*\)\n\n/, "");
const toolResults = request.messages.filter(message => message.role === "tool");
let content;
let stopReason = "stop";
{SUMMARY_TURN} else if (prompt === "Read proof." && toolResults.length === 0) {{
  content = [{{
    type: "tool_call", id: "read-proof", name: "read_file",
    arguments: {{ path: "proof.txt" }}
  }}];
  stopReason = "tool_use";
}} else if (prompt === "Read proof.") {{
  content = [{{ type: "text", text: "The durable proof was read." }}];
{HELD_TOOL_TURN}{STREAMED_TURN} else if (prompt === "Hold through reconnect.") {{
  writeFileSync(join(workspace, "model-pid"), String(process.pid));
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
{SPEAKER_TURNS}{COMPACTION_TURNS}{AUTOMATION_TURNS}{AGENT_TURNS}}} else if (prompt === "First.") {{
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

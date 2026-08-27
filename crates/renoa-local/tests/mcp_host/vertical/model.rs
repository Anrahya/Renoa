use std::{fs, path::Path};

use serde_json::Value;

pub(super) fn write_model_bridge(path: &Path, requests: &Path) {
    let mut source = format!(
        r#"
import {{ appendFileSync }} from "node:fs";
import {{ createHash as hash }} from "node:crypto";
let input = "";
for await (const chunk of process.stdin) input += chunk;
const action = process.env.RENOA_MODEL_ACTION;
const modelSpec = JSON.stringify({{ id: "fixture-model" }});
if (action === "catalog") {{
  process.stdout.write(JSON.stringify({{ ok: true, response: {{ models: [{{
    id: "fixture-model",
    name: "Fixture Model",
    reasoning_levels: ["high"],
    context_window_tokens: 500000,
    model_spec: {{ id: "fixture-model" }}
  }}] }} }}));
  process.exit(0);
}}
if (action === "describe") {{
  process.stdout.write(JSON.stringify({{ ok: true, response: {{
    context_window_tokens: 500000,
    max_output_tokens: 32768,
    model_spec: modelSpec,
    model_binding_id: hash("sha256").update(modelSpec).digest("hex"),
    reasoning_level: "high"
  }} }}));
  process.exit(0);
}}
if (action !== "stream") process.exit(2);
const request = JSON.parse(input);
appendFileSync({}, JSON.stringify(request) + "\n");
"#,
        serde_json::to_string(requests).expect("encode model request log path")
    );
    source.push_str(STREAM_BEHAVIOR);
    fs::write(path, source).expect("write model bridge");
}

const STREAM_BEHAVIOR: &str = r#"
const promptIndex = request.messages.findLastIndex(message => message.role === "user");
const prompt = request.messages[promptIndex].content[0].text;
const toolResults = request.messages.slice(promptIndex + 1).filter(message => message.role === "tool");
let content;
let stopReason;
if (toolResults.length === 0) {
  content = [{
    type: "tool_call",
    id: `mcp-search-${prompt.includes("denied") ? "denied" : prompt.includes("lost") ? "lost" : "ok"}`,
    name: "tool_search",
    arguments: { query: "echo" }
  }];
  stopReason = "tool_use";
} else if (toolResults.length === 1 && toolResults[0].result.name === "tool_search") {
  const search = JSON.parse(toolResults[0].result.content[0].text);
  if (
    toolResults[0].result.details !== null ||
    search.total_matches !== 1 ||
    search.matches.length !== 1 ||
    search.matches[0].name !== "echo" ||
    "input_schema" in search.matches[0]
  ) process.exit(3);
  content = [{
    type: "tool_call",
    id: `mcp-load-${prompt.includes("denied") ? "denied" : prompt.includes("lost") ? "lost" : "ok"}`,
    name: "tool_load",
    arguments: { references: [search.matches[0].reference] }
  }];
  stopReason = "tool_use";
} else if (toolResults.length === 2 && toolResults[1].result.name === "tool_load") {
  const loaded = JSON.parse(toolResults[1].result.content[0].text);
  if (
    toolResults[1].result.details !== null ||
    loaded.tools.length !== 1 ||
    loaded.tools[0].name !== "echo" ||
    loaded.tools[0].input_schema.required.join(",") !== "tenant,text" ||
    "x-mcp-header" in loaded.tools[0].input_schema.properties.tenant
  ) process.exit(3);
  const denied = prompt === "Use the denied echo tool.";
  const lost = prompt === "Use the lost echo tool.";
  content = [{
    type: "tool_call",
    id: lost ? "mcp-execute-lost" : denied ? "mcp-execute-denied" : "mcp-execute-ok",
    name: "tool_execute",
    arguments: {
      reference: loaded.tools[0].reference,
      arguments: { tenant: "alpha", text: lost ? "lost" : denied ? "denied" : "hello" }
    }
  }];
  stopReason = "tool_use";
} else if (
  prompt === "Use the echo tool." &&
  toolResults.length === 3 &&
  toolResults[2].result.name === "tool_execute" &&
  toolResults[2].result.content[0].text === "echo: hello" &&
  toolResults[2].result.details === null &&
  toolResults[2].result.is_error === false
) {
  content = [{ type: "text", text: "Echo completed." }];
  stopReason = "stop";
} else if (
  prompt === "Use the denied echo tool." &&
  toolResults.length === 3 &&
  toolResults[2].result.name === "tool_execute" &&
  toolResults[2].result.content[0].text === "permission denied" &&
  toolResults[2].result.details === null &&
  toolResults[2].result.is_error === true
) {
  content = [{ type: "text", text: "MCP error handled." }];
  stopReason = "stop";
} else {
  process.exit(3);
}
process.stdout.write(JSON.stringify({
  event: "completed",
  response: {
    content,
    stop_reason: stopReason,
    usage: { input: 10, output: 2, cache_read: 0, cache_write: 0 },
    metadata: { api: "test", provider: "xai", model: "fixture-model" }
  }
}) + "\n");
"#;

pub(super) fn read_json_lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .expect("read captured model requests")
        .lines()
        .map(|line| serde_json::from_str(line).expect("decode captured model request"))
        .collect()
}

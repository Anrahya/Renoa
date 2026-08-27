use std::{fs, path::Path};

use serde_json::Value;

pub(super) fn write_model_bridge(path: &Path, requests: &Path) {
    let source = format!(
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
const promptIndex = request.messages.findLastIndex(message => message.role === "user");
const prompt = request.messages[promptIndex].content[0].text;
const toolResults = request.messages.slice(promptIndex + 1).filter(message => message.role === "tool");
let content;
let stopReason;
if (toolResults.length === 0) {{
  const denied = prompt === "Use the denied echo tool.";
  const lost = prompt === "Use the lost echo tool.";
  content = [{{
    type: "tool_call",
    id: lost ? "mcp-echo-lost" : denied ? "mcp-echo-denied" : "mcp-echo-1",
    name: "echo",
    arguments: {{ tenant: "alpha", text: lost ? "lost" : denied ? "denied" : "hello" }}
  }}];
  stopReason = "tool_use";
}} else if (
  prompt === "Use the echo tool." &&
  toolResults.length === 1 &&
  toolResults[0].result.name === "echo" &&
  toolResults[0].result.content[0].text === "echo: hello" &&
  toolResults[0].result.details === null &&
  toolResults[0].result.is_error === false
) {{
  content = [{{ type: "text", text: "Echo completed." }}];
  stopReason = "stop";
}} else if (
  prompt === "Use the denied echo tool." &&
  toolResults.length === 1 &&
  toolResults[0].result.name === "echo" &&
  toolResults[0].result.content[0].text === "permission denied" &&
  toolResults[0].result.details === null &&
  toolResults[0].result.is_error === true
) {{
  content = [{{ type: "text", text: "MCP error handled." }}];
  stopReason = "stop";
}} else {{
  process.exit(3);
}}
process.stdout.write(JSON.stringify({{
  event: "completed",
  response: {{
    content,
    stop_reason: stopReason,
    usage: {{ input: 10, output: 2, cache_read: 0, cache_write: 0 }},
    metadata: {{ api: "test", provider: "xai", model: "fixture-model" }}
  }}
}}) + "\n");
"#,
        serde_json::to_string(requests).expect("encode model request log path")
    );
    fs::write(path, source).expect("write model bridge");
}

pub(super) fn read_json_lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .expect("read captured model requests")
        .lines()
        .map(|line| serde_json::from_str(line).expect("decode captured model request"))
        .collect()
}

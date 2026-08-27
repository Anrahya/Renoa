import { createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
let input = "";
for await (const chunk of process.stdin) input += chunk;
const provider = process.env.RENOA_MODEL_PROVIDER;
const models = provider === "opencode-go"
  ? [
      {
        id: "deepseek-test",
        name: "DeepSeek Test",
        reasoning_levels: ["low", "high"],
        context_window_tokens: 500000,
        model_spec: { id: "deepseek-test" }
      },
      {
        id: "grok-test",
        name: "Grok Test",
        reasoning_levels: ["off", "high"],
        context_window_tokens: 500000,
        model_spec: { id: "grok-test" }
      }
    ]
  : [
      {
        id: "grok-test",
        name: "Grok Test",
        reasoning_levels: ["low", "medium", "high"],
        context_window_tokens: 500000,
        model_spec: { id: "grok-test" }
      },
      {
        id: "grok-fast",
        name: "Grok Fast",
        reasoning_levels: ["off", "low", "high"],
        context_window_tokens: 500000,
        model_spec: { id: "grok-fast" }
      }
    ];
if (process.env.RENOA_MODEL_ACTION === "catalog") {
  process.stdout.write(JSON.stringify({ ok: true, response: { models } }));
  process.exit(0);
}
if (process.env.RENOA_MODEL_ACTION === "describe") {
  const modelSpec = process.env.RENOA_MODEL_SPEC;
  if (modelSpec !== JSON.stringify({ id: process.env.RENOA_MODEL })) {
    process.stdout.write(JSON.stringify({ ok: false, error: "selected model was not pinned" }));
    process.exit(0);
  }
  process.stdout.write(JSON.stringify({
    ok: true,
    response: {
      context_window_tokens: 500000,
      max_output_tokens: 500000,
      model_spec: modelSpec,
      model_binding_id: createHash("sha256").update(modelSpec).digest("hex"),
      reasoning_level: process.env.RENOA_MODEL_REASONING ?? "high"
    }
  }));
  process.exit(0);
}
const request = JSON.parse(input);
process.stdout.write(JSON.stringify({
  event: "provider_request",
  payload: {
    model: process.env.RENOA_MODEL,
    system_prompt: request.system_prompt,
    messages: request.messages,
    tools: request.tools
  }
}) + "\n");
process.stdout.write(JSON.stringify({
  event: "provider_response",
  status: 200,
  headers: { "x-request-id": "fixture-request" }
}) + "\n");
if (request.system_prompt.startsWith("You create durable context checkpoints")) {
  const attemptPath = process.env.RENOA_TEST_COMPACTION_ATTEMPTS;
  const attempts = existsSync(attemptPath)
    ? Number.parseInt(readFileSync(attemptPath, "utf8"), 10)
    : 0;
  writeFileSync(attemptPath, String(attempts + 1));
  const encoded = JSON.stringify(request.messages);
  if (
    request.tools.length !== 0 ||
    encoded.includes("/compact") ||
    !encoded.includes("First") ||
    !encoded.includes("First response.")
  ) {
    process.exit(4);
  }
  process.stdout.write(JSON.stringify({
    ok: true,
    response: {
      content: [{
        type: "text",
        text: "## Goal and user intent\nContinue the coding task.\n## Hard constraints and preferences\nPreserve durable facts.\n## Completed work\nAnswered the first prompt.\n## Current state and blockers\nNo blocker.\n## Decisions and rationale\nUse a durable checkpoint.\n## Exact working facts\nThe first response is durable.\n## Next action and unresolved questions\nAwait the next prompt."
      }],
      stop_reason: "stop",
      usage: { input: 3, output: 2, cache_read: 0, cache_write: 0 },
      metadata: { api: "test", provider, model: process.env.RENOA_MODEL }
    }
  }));
  process.exit(0);
}
const prompt = request.messages.findLast(message => message.role === "user").content[0].text;
const toolResults = request.messages.filter(message => message.role === "tool");
if (prompt === "FailContext") {
  process.stdout.write(JSON.stringify({
    event: "error",
    error: "prompt is too long for the context window",
    error_kind: "context_window_exceeded",
    inference_outcome: "known_not_started"
  }) + "\n");
  process.exit(0);
}
if (prompt === "FailProvider") {
  process.stdout.write(JSON.stringify({
    event: "error",
    error: "xAI request failed after 3 attempts: connection reset before an HTTP response (ECONNRESET).",
    error_kind: "network",
    inference_outcome: "known_not_started",
    diagnostic: {
      provider: "xai",
      model: process.env.RENOA_MODEL,
      attempt_count: 3,
      cause_code: "ECONNRESET",
      cause_message: "read ECONNRESET"
    }
  }) + "\n");
  process.exit(0);
}
if (prompt === "FailAfterDispatch") {
  process.stdout.write(JSON.stringify({
    event: "retry_attempt",
    attempt: 1,
    next_attempt: 2,
    category: "network",
    delay_ms: 0,
    cause_code: "ECONNRESET"
  }) + "\n");
  process.stdout.write(JSON.stringify({
    event: "error",
    error: "xAI request failed after 3 attempts: connection reset after the request may have been transmitted (ECONNRESET).",
    error_kind: "network",
    inference_outcome: "unknown",
    diagnostic: {
      provider: "xai",
      model: process.env.RENOA_MODEL,
      attempt_count: 3,
      cause_code: "ECONNRESET",
      cause_message: "socket destroyed after a complete request",
      provider_message: "The upstream closed the connection after reading the chat completion request."
    }
  }) + "\n");
  process.exit(0);
}
if (prompt === "Stream") {
  process.stdout.write(JSON.stringify({
    event: "content_delta",
    content_index: 0,
    delta: { type: "text", text: "Hello " }
  }) + "\n");
  while (!existsSync(process.env.RENOA_TEST_CONTINUE)) {
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  writeFileSync(process.env.RENOA_TEST_COMPLETED, "completed");
  process.stdout.write(JSON.stringify({
    event: "content_delta",
    content_index: 0,
    delta: { type: "text", text: "world" }
  }) + "\n");
  process.stdout.write(JSON.stringify({
    event: "completed",
    response: {
      content: [{ type: "text", text: "Hello world" }],
      stop_reason: "stop",
      usage: { input: 1, output: 2, cache_read: 0, cache_write: 0 },
      metadata: { api: "test", provider: "xai", model: process.env.RENOA_MODEL }
    }
  }) + "\n");
  process.exit(0);
}
let content;
let stop_reason = "stop";
if (prompt === "Hello") {
  content = [{ type: "text", text: "Hello back." }];
} else if (
  prompt === "Alpha" &&
  request.system_prompt.startsWith("You are Alpha, Renoa's local coding agent.") &&
  request.system_prompt.includes("Keep the ACP kernel path exact.") &&
  request.tools.map(tool => tool.name).join(",") ===
    "read_file,edit_file,write_file,bash,grep,find,tool_search,tool_load,tool_execute"
) {
  content = [{ type: "text", text: "Alpha is kernel-backed." }];
} else if (
  prompt === "Refresh instructions" &&
  request.system_prompt.includes("Use the replacement instruction.") &&
  !request.system_prompt.includes("Use the first instruction.")
) {
  content = [{ type: "text", text: "Instructions refreshed." }];
} else if (prompt === "Configured" && process.env.RENOA_MODEL_REASONING === "low") {
  content = [{ type: "text", text: "Reasoning configured." }];
} else if (
  prompt === "Model configured" &&
  process.env.RENOA_MODEL === "grok-fast" &&
  process.env.RENOA_MODEL_REASONING === "low"
) {
  content = [{ type: "text", text: "Model configured." }];
} else if (
  prompt === "OpenCode configured" &&
  provider === "opencode-go" &&
  process.env.RENOA_MODEL === "deepseek-test" &&
  process.env.RENOA_MODEL_REASONING === "high"
) {
  content = [{ type: "text", text: "OpenCode configured." }];
} else if (prompt === "First") {
  content = [{ type: "text", text: "First response." }];
} else if (
  prompt === "Second" &&
  request.messages.some(message =>
    message.role === "assistant" &&
    message.content.some(content => content.type === "text" && content.text === "First response.")
  )
) {
  content = [{ type: "text", text: "Continued from durable history." }];
} else if (prompt === "Wait") {
  writeFileSync(process.env.RENOA_TEST_STARTED, "started");
  await new Promise(resolve => setTimeout(resolve, 5000));
  writeFileSync(process.env.RENOA_TEST_COMPLETED, "completed");
  content = [{ type: "text", text: "Too late." }];
} else if (prompt === "Idempotent") {
  try {
    writeFileSync(process.env.RENOA_TEST_INVOKED, "invoked", { flag: "wx" });
  } catch {
    process.exit(3);
  }
  content = [{ type: "text", text: "Exactly once." }];
} else if (prompt === "Crash model") {
  const attemptsPath = process.env.RENOA_TEST_MODEL_ATTEMPTS;
  const attempts = existsSync(attemptsPath)
    ? Number.parseInt(readFileSync(attemptsPath, "utf8"), 10)
    : 0;
  writeFileSync(attemptsPath, String(attempts + 1));
  if (attempts === 0) {
    writeFileSync(process.env.RENOA_TEST_MODEL_CHILD_PID, String(process.pid));
    writeFileSync(process.env.RENOA_TEST_STARTED, "started");
    while (!existsSync(process.env.RENOA_TEST_CONTINUE)) {
      await new Promise(resolve => setTimeout(resolve, 10));
    }
  }
  content = [{ type: "text", text: "Recovered model call." }];
} else if (prompt === "Crash bash" && toolResults.length === 0) {
  content = [{
    type: "tool_call",
    id: "unsafe-bash-1",
    name: "bash",
    arguments: {
      command: "echo $$ > \"$RENOA_TEST_UNSAFE_CHILD_PID\"; echo started >> \"$RENOA_TEST_UNSAFE_STARTED\"; while [ ! -e \"$RENOA_TEST_UNSAFE_CONTINUE\" ]; do sleep 0.01; done; echo completed >> \"$RENOA_TEST_UNSAFE_COMPLETED\""
    }
  }];
  stop_reason = "tool_use";
} else if (prompt === "Crash bash" && toolResults.length === 1) {
  content = [{ type: "text", text: "Bash completed." }];
} else if (
  prompt === "Image" &&
  request.messages.findLast(message => message.role === "user").content[1].type === "image" &&
  request.messages.findLast(message => message.role === "user").content[1].data === "AAEC" &&
  request.messages.findLast(message => message.role === "user").content[1].mime_type === "image/png"
) {
  content = [{ type: "text", text: "Image received." }];
} else if (prompt === "Tool" && toolResults.length === 0) {
  process.stdout.write(JSON.stringify({
    event: "content_delta",
    content_index: 0,
    delta: { type: "text", text: "Checking. " }
  }) + "\n");
  content = [
    { type: "text", text: "Checking. " },
    {
      type: "tool_call",
      id: "read-1",
      name: "read_file",
      arguments: { path: "value.txt" }
    }
  ];
  stop_reason = "tool_use";
} else if (prompt === "Tool" && toolResults.length === 1) {
  content = [{ type: "text", text: "Read it." }];
} else {
  process.exit(2);
}
process.stdout.write(JSON.stringify({
  ok: true,
  response: {
    content,
    stop_reason,
    usage: { input: 1, output: 1, cache_read: 0, cache_write: 0 },
    metadata: { api: "test", provider, model: process.env.RENOA_MODEL }
  }
}));

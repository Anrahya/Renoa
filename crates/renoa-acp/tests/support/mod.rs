use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

use serde_json::{Value, json};

pub(crate) struct AcpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl AcpProcess {
    pub(crate) fn spawn(
        workspace: &std::path::Path,
        data: &std::path::Path,
        bridge: &std::path::Path,
        auth_store: &std::path::Path,
    ) -> Self {
        Self::spawn_with_providers(
            workspace,
            data,
            bridge,
            auth_store,
            "xai",
            "xai",
            "grok-test",
        )
    }

    pub(crate) fn spawn_with_providers(
        workspace: &std::path::Path,
        data: &std::path::Path,
        bridge: &std::path::Path,
        auth_store: &std::path::Path,
        providers: &str,
        default_provider: &str,
        default_model: &str,
    ) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_renoa-agent"))
            .arg("acp")
            .current_dir(workspace)
            .env("RENOA_DATA_DIR", data)
            .env("RENOA_MODEL_BRIDGE", bridge)
            .env("RENOA_MODEL_PROVIDERS", providers)
            .env("RENOA_MODEL_PROVIDER", default_provider)
            .env("RENOA_MODEL", default_model)
            .env("RENOA_MODEL_AUTH_STORE", auth_store)
            .env("RENOA_TEST_STARTED", data.join("model-started"))
            .env("RENOA_TEST_COMPLETED", data.join("model-completed"))
            .env("RENOA_TEST_CONTINUE", data.join("model-continue"))
            .env("RENOA_TEST_INVOKED", data.join("model-invoked"))
            .env("RENOA_TEST_MODEL_ATTEMPTS", data.join("model-attempts"))
            .env("RENOA_TEST_MODEL_CHILD_PID", data.join("model-child-pid"))
            .env("RENOA_TEST_UNSAFE_STARTED", data.join("unsafe-started"))
            .env("RENOA_TEST_UNSAFE_CHILD_PID", data.join("unsafe-child-pid"))
            .env("RENOA_TEST_UNSAFE_CONTINUE", data.join("unsafe-continue"))
            .env("RENOA_TEST_UNSAFE_COMPLETED", data.join("unsafe-completed"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start ACP process");
        let stdin = child.stdin.take().expect("ACP stdin");
        let stdout = BufReader::new(child.stdout.take().expect("ACP stdout"));
        Self {
            child,
            stdin: Some(stdin),
            stdout,
        }
    }

    pub(crate) fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("open ACP stdin");
        serde_json::to_writer(&mut *stdin, &message).expect("encode ACP request");
        stdin.write_all(b"\n").expect("terminate ACP request");
        stdin.flush().expect("flush ACP request");
    }

    pub(crate) fn initialize(&mut self) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": { "name": "renoa-test", "version": "0.0.0" }
            }
        }));
        self.read()
    }

    pub(crate) fn create_session(&mut self, workspace: &std::path::Path) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/new",
            "params": { "cwd": workspace, "mcpServers": [] }
        }));
        self.read()
    }

    pub(crate) fn load_session(
        &mut self,
        workspace: &std::path::Path,
        session_id: &str,
    ) -> (Vec<Value>, Value) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "session/load",
            "params": {
                "sessionId": session_id,
                "cwd": workspace,
                "mcpServers": []
            }
        }));
        let mut updates = Vec::new();
        loop {
            let message = self.read();
            if message["id"] == 2 {
                return (updates, message);
            }
            updates.push(message);
        }
    }

    pub(crate) fn prompt(&mut self, session_id: &str, text: &str, turn_id: &str) -> (Value, Value) {
        self.send_prompt(session_id, text, turn_id);
        let mut messages = self.read_until_response(3);
        let response = messages.pop().expect("prompt response");
        let update_position = messages
            .iter()
            .position(|message| {
                message["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
            })
            .expect("simple prompt update");
        let update = messages.remove(update_position);
        assert!(
            messages.len() <= 1
                && messages.iter().all(|message| {
                    message["params"]["update"]["sessionUpdate"] == "usage_update"
                }),
            "simple prompt returned an unexpected ACP message sequence: {messages:?}"
        );
        (update, response)
    }

    pub(crate) fn send_prompt_id(
        &mut self,
        request_id: u64,
        session_id: &str,
        text: &str,
        turn_id: &str,
    ) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "session/prompt",
            "params": {
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": text }],
                "_meta": { "requestId": turn_id, "promptId": turn_id }
            }
        }));
    }

    pub(crate) fn send_prompt(&mut self, session_id: &str, text: &str, turn_id: &str) {
        self.send_prompt_id(3, session_id, text, turn_id);
    }

    pub(crate) fn read_until_response(&mut self, request_id: u64) -> Vec<Value> {
        let mut messages = Vec::new();
        loop {
            let message = self.read();
            let is_response = message["id"] == request_id;
            messages.push(message);
            if is_response {
                return messages;
            }
        }
    }

    pub(crate) fn read(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read ACP response");
        assert!(!line.is_empty(), "ACP process closed stdout");
        serde_json::from_str(&line).expect("decode ACP response")
    }

    pub(crate) fn finish(mut self) {
        drop(self.stdin.take());
        let output = self.child.wait_with_output().expect("wait for ACP process");
        assert!(
            output.status.success(),
            "ACP process failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[allow(
        dead_code,
        reason = "shared integration-test support is compiled by tests that do not simulate a crash"
    )]
    pub(crate) fn kill(mut self) {
        drop(self.stdin.take());
        self.child.kill().expect("kill ACP process");
        let status = self.child.wait().expect("reap killed ACP process");
        assert!(!status.success(), "killed ACP process exited successfully");
    }
}

pub(crate) const BRIDGE: &str = r#"
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
    "read_file,edit_file,write_file,bash,grep,find"
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
"#;

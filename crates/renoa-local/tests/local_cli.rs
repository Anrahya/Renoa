use std::{fs, path::Path, process::Command};

use renoa_kernel::{EffectStatus, Kernel, OperationStatus, SessionId};
use tempfile::tempdir;
use uuid::Uuid;

#[test]
fn the_headless_runner_completes_a_durable_coding_turn() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let database = directory.path().join("kernel.sqlite3");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(workspace.join("value.txt"), "old\n").expect("write fixture");
    fs::write(
        workspace.join("AGENTS.md"),
        "Keep the fixture's trailing newline.\n",
    )
    .expect("write project instructions");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(&bridge, BRIDGE).expect("write model bridge");

    let output = Command::new(env!("CARGO_BIN_EXE_renoa-local"))
        .args([
            database.as_os_str(),
            workspace.as_os_str(),
            "new".as_ref(),
            "Change value.txt from old to new and verify it.".as_ref(),
        ])
        .env("RENOA_PI_BRIDGE", &bridge)
        .env("RENOA_PI_PROVIDER", "xai")
        .env("RENOA_PI_MODEL", "grok-test-a")
        .env("RENOA_PI_AUTH_STORE", &auth_store)
        .env("RENOA_PI_REASONING", "low")
        .output()
        .expect("run headless Renoa");

    assert!(
        output.status.success(),
        "runner failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 runner output");
    assert!(stdout.contains("Updated and verified."));
    let session = stdout
        .lines()
        .find_map(|line| line.strip_prefix("session_id="))
        .expect("runner reported session ID");
    assert_eq!(
        fs::read_to_string(workspace.join("value.txt")).expect("read edited file"),
        "new\n"
    );
    assert!(database.exists());

    let continuation = Command::new(env!("CARGO_BIN_EXE_renoa-local"))
        .args([
            database.as_os_str(),
            workspace.as_os_str(),
            session.as_ref(),
            "Continue with the existing context.".as_ref(),
        ])
        .env("RENOA_PI_BRIDGE", &bridge)
        .env("RENOA_PI_PROVIDER", "xai")
        .env("RENOA_PI_MODEL", "grok-test-b")
        .env("RENOA_PI_AUTH_STORE", &auth_store)
        .env("RENOA_PI_REASONING", "high")
        .output()
        .expect("resume headless Renoa");
    assert!(
        continuation.status.success(),
        "runner continuation failed: {}",
        String::from_utf8_lossy(&continuation.stderr)
    );
    assert!(
        String::from_utf8(continuation.stdout)
            .expect("UTF-8 continuation output")
            .contains("Continued with the prior model history.")
    );

    assert_frozen_runtime_selections(&database, session);
}

#[test]
fn invalid_reasoning_fails_before_the_provider_process_starts() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let bridge = directory.path().join("bridge.mjs");
    let marker = directory.path().join("provider-started");
    let auth_store = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(
        &bridge,
        format!(
            "import {{ writeFileSync }} from 'node:fs';\nwriteFileSync({}, 'started');\n",
            serde_json::to_string(&marker).expect("encode marker path")
        ),
    )
    .expect("write marker bridge");

    let output = Command::new(env!("CARGO_BIN_EXE_renoa-local"))
        .args([
            directory.path().join("kernel.sqlite").as_os_str(),
            workspace.as_os_str(),
            "new".as_ref(),
            "Do not dispatch.".as_ref(),
        ])
        .env("RENOA_PI_BRIDGE", &bridge)
        .env("RENOA_PI_PROVIDER", "xai")
        .env("RENOA_PI_MODEL", "grok-test")
        .env("RENOA_PI_AUTH_STORE", &auth_store)
        .env("RENOA_PI_REASONING", "impossible")
        .output()
        .expect("run headless Renoa with invalid reasoning");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("RENOA_PI_REASONING must be off, minimal, low, medium, high, xhigh, or max")
    );
    assert!(
        !marker.exists(),
        "invalid configuration must fail before provider startup"
    );
}

#[test]
fn authentication_failure_is_clear_and_does_not_block_the_session() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let bridge = directory.path().join("bridge.mjs");
    let database = directory.path().join("kernel.sqlite");
    let auth_store = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(&bridge, AUTHENTICATION_FAILURE_BRIDGE).expect("write authentication bridge");

    let output = Command::new(env!("CARGO_BIN_EXE_renoa-local"))
        .args([
            database.as_os_str(),
            workspace.as_os_str(),
            "new".as_ref(),
            "Attempt one model request.".as_ref(),
        ])
        .env("RENOA_PI_BRIDGE", &bridge)
        .env("RENOA_PI_PROVIDER", "xai")
        .env("RENOA_PI_MODEL", "grok-test")
        .env("RENOA_PI_AUTH_STORE", &auth_store)
        .output()
        .expect("run headless Renoa with rejected authentication");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("model authentication failed: OAuth refresh failed for xai"));
    assert!(!stderr.contains("blocked on an uncertain"));
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 runner output");
    let session = stdout
        .lines()
        .find_map(|line| line.strip_prefix("session_id="))
        .expect("runner reported session ID");
    let kernel = Kernel::open(&database).expect("reopen kernel");
    let session_id = SessionId::from_uuid(Uuid::parse_str(session).expect("valid session UUID"));
    let snapshot = kernel.inspect(session_id).expect("inspect failed session");
    assert_eq!(snapshot.operations[0].status, OperationStatus::Failed);
    assert_eq!(
        snapshot.operations[0].effects[0].status,
        EffectStatus::Settled
    );
}

fn assert_frozen_runtime_selections(database: &Path, session: &str) {
    let kernel = Kernel::open(database).expect("reopen kernel");
    let session_id = SessionId::from_uuid(Uuid::parse_str(session).expect("valid session UUID"));
    let snapshot = kernel.inspect(session_id).expect("inspect session");
    assert_eq!(snapshot.operations.len(), 2);
    let first = snapshot.operations[0]
        .manifest
        .as_ref()
        .expect("first operation manifest");
    let second = snapshot.operations[1]
        .manifest
        .as_ref()
        .expect("second operation manifest");
    assert_eq!(
        first
            .effect_bindings
            .get("renoa.agent.model")
            .map(String::as_str),
        Some(
            "pi/xai/grok-test-a/44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a/reasoning-low"
        )
    );
    assert_eq!(
        second
            .effect_bindings
            .get("renoa.agent.model")
            .map(String::as_str),
        Some(
            "pi/xai/grok-test-b/44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a/reasoning-high"
        )
    );
    assert_eq!(
        first.config_digest, second.config_digest,
        "changing the model selection must not change Alpha's profile configuration"
    );
}

const BRIDGE: &str = r#"
let input = "";
for await (const chunk of process.stdin) input += chunk;
if (process.env.RENOA_PI_ACTION === "describe") {
  process.stdout.write(JSON.stringify({
    ok: true,
    response: {
      context_window_tokens: 500000,
      max_output_tokens: 500000,
      model_spec: "{}",
      model_binding_id: "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
      reasoning_level: process.env.RENOA_PI_REASONING ?? "high"
    }
  }));
  process.exit(0);
}
const request = JSON.parse(input);
if (!request.system_prompt.startsWith("You are Alpha, Renoa's local coding agent.")) {
  throw new Error("Alpha base prompt was not sent");
}
if (!request.system_prompt.includes("Keep the fixture's trailing newline.")) {
  throw new Error("project instructions were not sent");
}
if (request.system_prompt.includes("read_file") || request.system_prompt.includes("config_digest")) {
  throw new Error("tool schemas or runtime bookkeeping leaked into the system prompt");
}
const toolNames = request.tools.map((tool) => tool.name);
if (JSON.stringify(toolNames) !== JSON.stringify([
  "read_file", "edit_file", "write_file", "bash", "grep", "find"
])) {
  throw new Error(`unexpected Alpha tools: ${JSON.stringify(toolNames)}`);
}
if (process.env.RENOA_PI_MODEL === "grok-test-b") {
  const prior = request.messages.find(
    (message) => message.role === "assistant" && message.metadata?.model === "grok-test-a"
  );
  if (!prior) throw new Error("the prior model's history was not preserved");
  process.stdout.write(JSON.stringify({
    ok: true,
    response: {
      content: [{ type: "text", text: "Continued with the prior model history." }],
      stop_reason: "stop",
      usage: { input: 1, output: 1, cache_read: 0, cache_write: 0 },
      metadata: { api: "test", provider: "xai", model: "grok-test-b" }
    }
  }));
  process.exit(0);
}
const results = request.messages.filter((message) => message.role === "tool");
let content;
let stop_reason;
switch (results.length) {
  case 0:
    content = [{
      type: "tool_call",
      id: "read-1",
      name: "read_file",
      arguments: { path: "value.txt" }
    }];
    stop_reason = "tool_use";
    break;
  case 1:
    content = [{
      type: "tool_call",
      id: "edit-1",
      name: "edit_file",
      arguments: { path: "value.txt", old_text: "old\n", new_text: "new\n" }
    }];
    stop_reason = "tool_use";
    break;
  case 2:
    content = [{
      type: "tool_call",
      id: "bash-1",
      name: "bash",
      arguments: { command: "test \"$(cat value.txt)\" = new" }
    }];
    stop_reason = "tool_use";
    break;
  default:
    content = [{ type: "text", text: "Updated and verified." }];
    stop_reason = "stop";
}
process.stdout.write(JSON.stringify({
  ok: true,
  response: {
    content,
    stop_reason,
    usage: { input: 1, output: 1, cache_read: 0, cache_write: 0 },
    metadata: { api: "test", provider: "xai", model: "grok-test-a" }
  }
}));
"#;

const AUTHENTICATION_FAILURE_BRIDGE: &str = r#"
let input = "";
for await (const chunk of process.stdin) input += chunk;
if (process.env.RENOA_PI_ACTION === "describe") {
  process.stdout.write(JSON.stringify({
    ok: true,
    response: {
      context_window_tokens: 500000,
      max_output_tokens: 500000,
      model_spec: "{}",
      model_binding_id: "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
      reasoning_level: "high"
    }
  }));
  process.exit(0);
}
process.stdout.write(JSON.stringify({
  event: "error",
  error: "OAuth refresh failed for xai",
  error_kind: "authentication_failed"
}) + "\n");
"#;

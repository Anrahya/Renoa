use std::{fs, process::Command};

use tempfile::tempdir;

#[test]
fn the_headless_runner_completes_a_durable_coding_turn() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let database = directory.path().join("harness.sqlite3");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(workspace.join("value.txt"), "old\n").expect("write fixture");
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
        .env("RENOA_PI_MODEL", "grok-test")
        .env("RENOA_PI_AUTH_STORE", &auth_store)
        .env(
            "RENOA_PI_INSTRUCTIONS",
            "Edit carefully and verify the result.",
        )
        .output()
        .expect("run headless Renoa");

    assert!(
        output.status.success(),
        "runner failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 runner output");
    assert!(stdout.contains("Updated and verified."));
    assert!(stdout.contains("session_id="));
    assert_eq!(
        fs::read_to_string(workspace.join("value.txt")).expect("read edited file"),
        "new\n"
    );
    assert!(database.exists());
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
      reasoning_level: "high"
    }
  }));
  process.exit(0);
}
const request = JSON.parse(input);
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
    metadata: { api: "test", provider: "xai", model: "grok-test" }
  }
}));
"#;

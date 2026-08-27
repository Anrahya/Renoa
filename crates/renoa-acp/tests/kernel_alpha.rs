#[path = "support/assertions.rs"]
mod assertions;
mod support;

use std::fs;

use renoa_kernel::{Kernel, SessionId};
use tempfile::tempdir;
use uuid::Uuid;

use assertions::assert_equivalent_prompt_outcomes;
use support::{AcpProcess, BRIDGE};

#[test]
fn acp_runs_the_frozen_alpha_profile_through_the_kernel() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let data = directory.path().join("data");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(
        workspace.join("AGENTS.md"),
        "Keep the ACP kernel path exact.\n",
    )
    .expect("write project instructions");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(&bridge, BRIDGE).expect("write model bridge");
    let mut process = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    process.initialize();
    let created = process.create_session(&workspace);
    let session_id = created["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();

    let (update, completed) =
        process.prompt(&session_id, "Alpha", "804f8264-ce11-4fe9-84e4-fb29326548d3");

    assert_eq!(
        update["params"]["update"]["content"]["text"],
        "Alpha is kernel-backed."
    );
    assert_eq!(completed["result"]["stopReason"], "end_turn");
    process.finish();

    let kernel_path = data
        .join("sessions")
        .join(&session_id)
        .join("kernel.sqlite3");
    let kernel = Kernel::open(&kernel_path).expect("open ACP kernel database");
    let session_id = SessionId::from_uuid(Uuid::parse_str(&session_id).expect("session UUID"));
    let snapshot = kernel.inspect(session_id).expect("inspect Alpha session");
    let manifest = snapshot.operations[0]
        .manifest
        .as_ref()
        .expect("frozen Alpha manifest");
    assert_eq!(manifest.loop_binding, "renoa.agent.model-tool-loop");
    assert_eq!(manifest.effect_bindings.len(), 12);
    assert!(manifest.effect_bindings.contains_key("renoa.agent.model"));
    for tool in [
        "read_file",
        "edit_file",
        "write_file",
        "bash",
        "grep",
        "find",
        "tool_search",
        "tool_load",
        "tool_execute",
        "skill_search",
        "skill_load",
    ] {
        assert!(
            manifest
                .effect_bindings
                .contains_key(&format!("renoa.agent.tool/{tool}")),
            "missing frozen tool binding `{tool}`"
        );
    }
}

#[test]
fn each_new_turn_reads_the_current_workspace_instructions() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let data = directory.path().join("data");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(workspace.join("AGENTS.md"), "Use the first instruction.\n")
        .expect("write first project instructions");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(&bridge, BRIDGE).expect("write model bridge");
    let mut process = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    process.initialize();
    let created = process.create_session(&workspace);
    let session_id = created["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();

    fs::write(
        workspace.join("AGENTS.md"),
        "Use the replacement instruction.\n",
    )
    .expect("replace project instructions");
    let (update, completed) = process.prompt(
        &session_id,
        "Refresh instructions",
        "294375bc-a6ec-4a84-996a-b5766353ae61",
    );

    assert_eq!(
        update["params"]["update"]["content"]["text"],
        "Instructions refreshed."
    );
    assert_eq!(completed["result"]["stopReason"], "end_turn");
    process.finish();
}

#[test]
fn settled_redelivery_does_not_re_resolve_changed_workspace_instructions() {
    let directory = tempdir().expect("temporary directory");
    let workspace = directory.path().join("workspace");
    let data = directory.path().join("data");
    let bridge = directory.path().join("bridge.mjs");
    let auth_store = directory.path().join("auth.sqlite");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(
        workspace.join("AGENTS.md"),
        "Keep the ACP kernel path exact.\n",
    )
    .expect("write project instructions");
    fs::write(&auth_store, "").expect("create auth placeholder");
    fs::write(&bridge, BRIDGE).expect("write model bridge");
    let mut process = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    process.initialize();
    let created = process.create_session(&workspace);
    let session_id = created["result"]["sessionId"]
        .as_str()
        .expect("session id")
        .to_owned();
    let request_id = "161d87c6-a116-49c9-8f54-136b30e59cbc";
    let first = process.prompt(&session_id, "Alpha", request_id);
    process.finish();

    fs::write(workspace.join("AGENTS.md"), vec![b'x'; 32 * 1024 + 1])
        .expect("make current project instructions invalid");
    let mut resumed = AcpProcess::spawn(&workspace, &data, &bridge, &auth_store);
    resumed.initialize();
    let (_history, loaded) = resumed.load_session(&workspace, &session_id);
    assert!(loaded.get("result").is_some(), "load failed: {loaded}");
    let replay = resumed.prompt(&session_id, "Alpha", request_id);

    assert_equivalent_prompt_outcomes(&first, &replay, request_id);
    resumed.finish();
}

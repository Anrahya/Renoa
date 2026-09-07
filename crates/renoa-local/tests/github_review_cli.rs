use std::{collections::BTreeSet, fmt::Write as _, fs, path::Path, process::Command};

use renoa_kernel::AgentId;
use renoa_local::{
    AgentProfileId, AgentRecord, BotRecipe, BotRecord, GitHubReviewAdmission, GitHubReviewCommand,
    GitHubReviewPolicy, GitHubReviewReply, GitHubReviewTrigger, LocalHost, LocalHostAdapters,
    LocalModelConfiguration, ModelProvider, arcee_profile,
};
use uuid::Uuid;

fn invoke(root: &Path, mode: &str, request: &impl serde::Serialize) -> std::process::Output {
    let file = root.join("request.json");
    fs::write(&file, serde_json::to_vec(request).expect("encode request")).expect("request");
    Command::new(env!("CARGO_BIN_EXE_renoa-host"))
        .arg(root.join("host.json"))
        .arg(mode)
        .arg(file)
        .output()
        .expect("Host CLI")
}

fn reply(output: &std::process::Output) -> GitHubReviewReply {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("typed management reply")
}

fn fixture(root: &Path) -> AgentId {
    let data = root.join("data");
    let bridge = root.join("model.mjs");
    let auth = root.join("auth.sqlite");
    fs::write(
        &bridge,
        "throw new Error('review admission must not invoke inference');",
    )
    .expect("model");
    fs::write(&auth, "").expect("auth boundary");
    let model = LocalModelConfiguration::new(
        &bridge,
        vec![ModelProvider::Xai],
        ModelProvider::Xai,
        "fixture",
        &auth,
    );
    let host = LocalHost::new(
        &data,
        model,
        vec![arcee_profile(&data).expect("profile")],
        LocalHostAdapters::default(),
    )
    .expect("Host");
    let operator = AgentId::new();
    let specialist = AgentId::new();
    tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(async {
            host.ensure_agent(AgentRecord {
                id: operator,
                profile: AgentProfileId::new(renoa_local::ARCEE_PROFILE_ID).expect("profile ID"),
                name: "Arcee".to_owned(),
                created_by: None,
            })
            .await
            .expect("operator");
            host.ensure_bot(BotRecord {
                id: specialist,
                created_by: operator,
                recipe: BotRecipe {
                    name: "Review Desk".to_owned(),
                    instructions: "Read repository evidence".to_owned(),
                    tools: ["read_file".to_owned()].into(),
                    connections: BTreeSet::new(),
                },
            })
            .await
            .expect("specialist");
        });
    drop(host);
    fs::write(
        root.join("host.json"),
        serde_json::to_vec(&serde_json::json!({
            "data_directory":data,"model_bridge":bridge,"providers":["xai"],"provider":"xai",
            "model":"fixture","model_auth_store":auth
        }))
        .expect("config"),
    )
    .expect("config file");
    specialist
}

#[test]
fn separate_cli_processes_configure_admit_and_recover_the_same_review_request() {
    let directory = tempfile::tempdir().expect("directory");
    let root = directory.path();
    let specialist = fixture(root);
    let command = GitHubReviewCommand::SetRepository {
        operation_id: Uuid::new_v4(),
        expected_revision: None,
        policy: GitHubReviewPolicy {
            repository_id: 42,
            installation_id: 7,
            full_name: "owner/repo".to_owned(),
            agent_id: specialist,
            enabled: true,
            triggers: [GitHubReviewTrigger::Opened].into(),
            skip_drafts: true,
        },
    };
    let first = reply(&invoke(root, "github-review", &command));
    assert_eq!(first, reply(&invoke(root, "github-review", &command)));
    let secret = b"private CLI fixture secret";
    let secret_path = root.join("webhook-secret");
    fs::write(&secret_path, secret).expect("secret");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&secret_path, fs::Permissions::from_mode(0o600))
            .expect("private secret");
    }
    let body = serde_json::to_vec(
        &serde_json::json!({"action":"opened","installation":{"id":7},
        "repository":{"id":42},"pull_request":{"number":14,"state":"open","draft":false,
        "base":{"sha":"a".repeat(40)},"head":{"sha":"b".repeat(40)}}}),
    )
    .expect("payload");
    let body_path = root.join("webhook.json");
    fs::write(&body_path, &body).expect("body");
    let tag = ring::hmac::sign(
        &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret),
        &body,
    );
    let mut signature = "sha256=".to_owned();
    for byte in tag.as_ref() {
        write!(signature, "{byte:02x}").expect("signature hex");
    }
    let envelope = serde_json::json!({"delivery_id":Uuid::new_v4(),"event":"pull_request",
        "signature":signature,"body_file":body_path,"secret_file":secret_path});
    let output = invoke(root, "github-webhook", &envelope);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let admitted: GitHubReviewAdmission = serde_json::from_slice(&output.stdout).expect("admitted");
    let GitHubReviewAdmission::Queued { request_id } = admitted else {
        panic!("must queue")
    };
    assert_eq!(
        output.stdout,
        invoke(root, "github-webhook", &envelope).stdout
    );
    let GitHubReviewReply::Requests { records } = reply(&invoke(
        root,
        "github-review",
        &GitHubReviewCommand::Requests { after: 0 },
    )) else {
        panic!("request page")
    };
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].id, request_id);
    assert_eq!(records[0].repository.policy.agent_id, specialist);
    assert_eq!(records[0].reported_head_sha, "b".repeat(40));
    let mut wrong_signature = envelope.clone();
    wrong_signature["signature"] = "sha256=00".into();
    let rejected = invoke(root, "github-webhook", &wrong_signature);
    assert!(!rejected.status.success());
    assert!(!String::from_utf8_lossy(&rejected.stderr).contains("private CLI fixture secret"));
}

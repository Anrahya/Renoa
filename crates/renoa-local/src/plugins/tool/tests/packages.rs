use std::{
    fs,
    path::{Path, PathBuf},
};

use renoa_agent::{ContentBlock, Tool, ToolCall, invoke_tool};
use serde_json::{Value, json};
use tempfile::{TempDir, tempdir};
use tokio_util::sync::CancellationToken;

use super::{
    super::{ManageTool, TOOL_NAME},
    call,
};
use crate::{
    host::catalog,
    mcp::{McpCatalogStore, McpCredentialResolver},
    plugins::{PluginManager, tests::test_skill_store},
    skills::SkillStore,
};

#[tokio::test]
async fn one_agent_tool_inspects_installs_and_lists_an_exact_package() {
    let directory = tempdir().expect("temporary extension tool fixture");
    let database = directory.path().join("host.sqlite3");
    catalog::initialize(&database).expect("initialize Host catalog");
    let mcp = McpCatalogStore::open(database.clone()).expect("open MCP catalog");
    let skills = test_skill_store(&database, directory.path());
    let manager = PluginManager::initialize(
        database,
        directory.path().join("installed"),
        mcp.clone(),
        None,
        McpCredentialResolver::default(),
        None,
        skills,
    )
    .expect("initialize extension manager");
    let source = directory.path().join("source");
    fs::create_dir(&source).expect("create plugin source");
    fs::write(
        source.join("plugin.json"),
        serde_json::to_vec(&json!({
            "$schema": crate::plugins::inspect::PLUGIN_SCHEMA,
            "name": "fixture"
        }))
        .expect("encode manifest"),
    )
    .expect("write manifest");
    let tool = ManageTool::new(manager, directory.path().to_path_buf());
    let schema = &tool.spec().input_schema;
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["required"], json!(["action"]));
    assert_eq!(
        schema["properties"]["action"]["enum"],
        json!([
            "search",
            "add",
            "inspect",
            "install",
            "list",
            "connect",
            "authorize"
        ])
    );
    assert!(schema.get("oneOf").is_none());
    assert_eq!(schema["properties"]["source"]["type"], "object");
    assert_eq!(
        schema["properties"]["source"]["properties"]["kind"]["enum"],
        json!(["catalog", "mcp", "package"])
    );
    assert_eq!(
        schema["properties"]["credential"]["oneOf"][0]["required"],
        json!(["kind", "credential_id"])
    );
    assert_eq!(
        schema["properties"]["credential"]["oneOf"][1]["properties"]["kind"]["const"],
        "oauth"
    );
    assert!(schema["properties"].get("candidate").is_none());
    assert!(!schema.to_string().contains("connection_id"));

    let inspected = call(&tool, json!({"action": "inspect", "source_path": "source"})).await;
    let digest = inspected["digest"]
        .as_str()
        .expect("inspection returned digest");
    let installed = call(
        &tool,
        json!({
            "action": "install",
            "source_path": "source",
            "expected_digest": digest
        }),
    )
    .await;
    assert_eq!(installed["digest"], digest);
    let listed = call(&tool, json!({"action": "list"})).await;
    assert_eq!(listed.as_array().expect("installed package list").len(), 1);
    assert_eq!(listed[0]["metadata"]["name"], "fixture");
}

#[tokio::test]
async fn local_package_add_is_content_bound_and_replay_ignores_source_changes() {
    let fixture = LocalPackageFixture::new();
    let unbound = invoke_tool(
        Some(&fixture.tool),
        ToolCall {
            id: "unbound-package-add".to_owned(),
            name: TOOL_NAME.to_owned(),
            arguments: json!({
                "action": "add",
                "source": {"kind": "package", "source_path": "source"}
            }),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("a missing digest is a definite model-visible input error");
    assert!(unbound.is_error);
    let [ContentBlock::Text { text }] = unbound.content.as_slice() else {
        panic!("a missing digest must return one text error")
    };
    assert!(text.contains("expected_digest"));
    assert_eq!(
        unbound
            .details
            .as_ref()
            .and_then(|details| details["error"]["code"].as_str()),
        Some("invalid_input")
    );

    let digest = fixture.digest().await;
    let add = package_add(&digest, None);
    let added = call(&fixture.tool, add.clone()).await;
    fs::write(
        fixture.skill.join("SKILL.md"),
        "---\nname: review\ndescription: Mutated source.\n---\nMUTATED\n",
    )
    .expect("mutate the source after installation");
    let replay = call(&fixture.tool, add).await;
    assert_eq!(replay["package_digest"], added["package_digest"]);
    assert_eq!(
        fixture
            .skills
            .summaries(crate::ALPHA_PROFILE_ID, fixture.directory.path())
            .expect("read replayed plugin skill")[0]
            .description,
        "Review this code."
    );
}

#[tokio::test]
async fn package_add_reports_loaded_and_rejected_components_after_installation() {
    let fixture = LocalPackageFixture::new();
    let digest = fixture.digest().await;
    let added = call(&fixture.tool, package_add(&digest, None)).await;

    assert_eq!(added["status"], "installed");
    assert_eq!(added["source"], "package");
    assert_eq!(added["metadata"]["name"], "local-fixture");
    assert_eq!(added["notices"][0]["component"], "mcp");
    assert_eq!(added["notices"][0]["entry"], "local-helper");
    assert!(
        added["notices"][0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("not supported"))
    );
    assert_eq!(added["skills"]["accepted"], json!(["review"]));
    assert_eq!(
        added["skills"]["rejected"][0]["entry"],
        "implicit-permission"
    );
    assert!(
        added["skills"]["rejected"][0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("allowed-tools"))
    );
    assert_eq!(
        fixture
            .skills
            .summaries(crate::ALPHA_PROFILE_ID, fixture.directory.path())
            .expect("read hot-loaded plugin skill")[0]
            .name,
        "review"
    );

    let connection_failure = invoke_tool(
        Some(&fixture.tool),
        ToolCall {
            id: "missing-supported-server".to_owned(),
            name: TOOL_NAME.to_owned(),
            arguments: package_add(&digest, Some("local-fixture.default")),
            thought_signature: None,
            namespace: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("post-install connection selection failure is definite");
    assert!(connection_failure.is_error);
    let [ContentBlock::Text { text }] = connection_failure.content.as_slice() else {
        panic!("connection selection failure must be model-visible")
    };
    let error: Value = serde_json::from_str(text).expect("decode connection selection failure");
    assert_eq!(error["status"], "installed_connection_failed");
    assert_eq!(error["package_digest"], added["package_digest"]);
    assert_eq!(error["connection"], "local-fixture.default");
    assert!(error.get("server").is_none());
    assert_eq!(error["skills"]["accepted"], json!(["review"]));
}

struct LocalPackageFixture {
    directory: TempDir,
    skill: PathBuf,
    skills: SkillStore,
    tool: ManageTool,
}

impl LocalPackageFixture {
    fn new() -> Self {
        let directory = tempdir().expect("temporary local add fixture");
        let database = directory.path().join("host.sqlite3");
        catalog::initialize(&database).expect("initialize Host catalog");
        let mcp = McpCatalogStore::open(database.clone()).expect("open MCP catalog");
        let skills = test_skill_store(&database, directory.path());
        let manager = PluginManager::initialize(
            database,
            directory.path().join("installed"),
            mcp,
            None,
            McpCredentialResolver::default(),
            None,
            skills.clone(),
        )
        .expect("initialize extension manager");
        let source = directory.path().join("source");
        fs::create_dir(&source).expect("create plugin source");
        write_local_manifest(&source);
        let skill = write_local_components(&source);
        let tool = ManageTool::new(manager, directory.path().to_path_buf());
        Self {
            directory,
            skill,
            skills,
            tool,
        }
    }

    async fn digest(&self) -> String {
        let inspected = call(
            &self.tool,
            json!({"action": "inspect", "source_path": "source"}),
        )
        .await;
        inspected["digest"]
            .as_str()
            .expect("inspection returned a digest")
            .to_owned()
    }
}

fn write_local_manifest(source: &Path) {
    fs::write(
        source.join("plugin.json"),
        serde_json::to_vec(&json!({
            "$schema": crate::plugins::inspect::PLUGIN_SCHEMA,
            "name": "local-fixture"
        }))
        .expect("encode manifest"),
    )
    .expect("write manifest");
}

fn write_local_components(source: &Path) -> PathBuf {
    fs::write(
        source.join("mcp.json"),
        serde_json::to_vec(&json!({
            "$schema": crate::plugins::inspect::MCP_SCHEMA,
            "mcpServers": {
                "local-helper": {"type": "stdio", "command": "local-helper"}
            }
        }))
        .expect("encode unsupported MCP fixture"),
    )
    .expect("write unsupported MCP fixture");
    let skill = source.join("skills/review");
    fs::create_dir_all(&skill).expect("create plugin skill");
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: review\ndescription: Review this code.\n---\nReview carefully.\n",
    )
    .expect("write plugin skill");
    let rejected = source.join("skills/implicit-permission");
    fs::create_dir_all(&rejected).expect("create rejected plugin skill");
    fs::write(
        rejected.join("SKILL.md"),
        "---\nname: implicit-permission\ndescription: Invalid permission fixture.\nallowed-tools: Bash\n---\nDo not load.\n",
    )
    .expect("write rejected plugin skill");
    skill
}

fn package_add(digest: &str, connection: Option<&str>) -> Value {
    let mut request = json!({
        "action": "add",
        "source": {
            "kind": "package",
            "source_path": "source",
            "expected_digest": digest
        }
    });
    if let Some(connection) = connection {
        request["connection"] = Value::String(connection.to_owned());
    }
    request
}

use std::{
    fs,
    path::{Path, PathBuf},
};

use renoa_agent::{ContentBlock, ToolCall, invoke_tool};
use serde_json::{Value, json};
use tempfile::{TempDir, tempdir};
use tokio_util::sync::CancellationToken;

use super::{
    super::{
        ManageTool, TOOL_NAME,
        contract::{ManageInput, manage_tool_spec},
    },
    call,
};
use crate::{
    host::catalog,
    mcp::{McpCatalogStore, McpCredentialResolver},
    plugins::{PluginManager, tests::test_skill_store},
    skills::SkillStore,
};

#[test]
fn oauth_credential_uses_the_exact_public_spelling_and_requires_a_registration_mode() {
    serde_json::from_value::<ManageInput>(json!({
        "action": "connect",
        "package_digest": "a".repeat(64),
        "server": "drive",
        "connection": "drive",
        "credential": {
            "kind": "oauth",
            "registration": {"mode": "pre_registered", "credential_id": "drive.client"}
        }
    }))
    .expect("the documented oauth spelling must deserialize");
    assert!(
        serde_json::from_value::<ManageInput>(json!({
            "action": "connect",
            "package_digest": "a".repeat(64),
            "server": "drive",
            "connection": "drive",
            "credential": {"kind": "o_auth", "registration": {"mode": "dynamic"}}
        }))
        .is_err()
    );
}

#[test]
fn extension_schema_keeps_raw_credentials_out_of_the_agent_path() {
    let spec = manage_tool_spec(TOOL_NAME);
    assert!(spec.description.contains("references only"));
    assert!(spec.description.contains("never pass API keys"));
    assert!(spec.description.contains("untrusted data"));
    let schema = &spec.input_schema;
    assert_eq!(schema["type"], "object");
    assert!(schema.get("required").is_none());
    let variants = schema["oneOf"]
        .as_array()
        .expect("management schema has action variants");
    assert_eq!(
        variants
            .iter()
            .map(|variant| variant["properties"]["action"]["const"]
                .as_str()
                .expect("action variant has a string discriminator"))
            .collect::<Vec<_>>(),
        [
            "search",
            "lookup",
            "add",
            "inspect",
            "install",
            "list",
            "connect",
            "authorize",
            "disconnect",
            "enable"
        ]
    );
    assert!(
        variants
            .iter()
            .all(|variant| variant["additionalProperties"] == false)
    );
    let add = &variants[2];
    assert_eq!(add["required"], json!(["action", "source"]));
    assert_eq!(
        add["properties"]["source"]["oneOf"][0]["properties"]["kind"]["const"],
        "mcp"
    );
    assert_eq!(
        add["properties"]["credential"]["oneOf"][0]["required"],
        json!(["kind", "credential_id"])
    );
    assert_eq!(
        add["properties"]["credential"]["oneOf"][1]["properties"]["kind"]["const"],
        "secret_service_header"
    );
    assert_eq!(
        add["properties"]["credential"]["oneOf"][2]["required"],
        json!(["kind", "registration"])
    );
    assert_eq!(
        add["properties"]["credential"]["oneOf"][2]["properties"]["registration"]["oneOf"][2]["properties"]
            ["mode"]["const"],
        "pre_registered"
    );
    let credential_description = add["properties"]["credential"]["description"]
        .as_str()
        .expect("credential schema has model guidance");
    assert!(credential_description.contains("existing Host credential reference"));
    assert!(credential_description.contains("secret_service_header"));
    let encoded = schema.to_string();
    assert!(!encoded.contains("candidate"));
    assert!(encoded.contains("query"));
    assert!(!encoded.contains("connection_id"));
    assert_eq!(
        variants[1]["properties"]["registry_version"]["not"]["const"],
        "latest"
    );
    assert_eq!(variants[5]["required"], json!(["action"]));
    assert_eq!(
        variants[5]["properties"]
            .as_object()
            .map(serde_json::Map::len),
        Some(3)
    );
    assert_eq!(variants[5]["properties"]["limit"]["maximum"], 32);
    serde_json::from_value::<ManageInput>(json!({"action": "search", "query": "cloudflare"}))
        .expect("official Registry search is a typed action");
    serde_json::from_value::<ManageInput>(json!({
        "action": "lookup",
        "registry_name": "com.cloudflare.mcp/mcp",
        "registry_version": "1.0.0"
    }))
    .expect("official Registry exact lookup is a typed action");
    assert!(
        serde_json::from_value::<ManageInput>(json!({
            "action": "add",
            "source": {"kind": "catalog", "candidate": "stale-entry"}
        }))
        .is_err()
    );
}

#[test]
fn management_arguments_reject_fields_from_another_action() {
    assert!(
        serde_json::from_value::<ManageInput>(json!({
            "action": "list",
            "connection": "must-not-be-ignored"
        }))
        .is_err()
    );
    assert!(serde_json::from_value::<ManageInput>(json!({"action": "enable"})).is_err());
}

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
        None,
        McpCredentialResolver::default(),
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
    assert_eq!(listed["total"], 1);
    assert_eq!(listed["returned"], 1);
    assert_eq!(super::inventory_items(&listed)[0]["kind"], "package");
    assert_eq!(super::inventory_items(&listed)[0]["name"], "fixture");
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
    let listed = call(&fixture.tool, json!({"action": "list"})).await;
    let source = super::inventory_item(&listed, "plugin_skill_source");
    assert_eq!(source["source"], "agent-plugin:local-fixture");
    assert_eq!(source["accepted_count"], 1);
    assert_eq!(source["rejected_count"], 1);
    let accepted = super::inventory_item(&listed, "plugin_skill");
    assert_eq!(accepted["name"], "review");
    let rejected = super::inventory_item(&listed, "plugin_skill_rejection");
    assert_eq!(rejected["entry"], "implicit-permission");

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
            None,
            McpCredentialResolver::default(),
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

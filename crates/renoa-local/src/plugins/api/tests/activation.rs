use super::*;
use crate::{AgentId, plugins::PluginActivation};
use renoa_kernel::{CommandId, SessionId};
use serde_json::json;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use tokio_util::sync::CancellationToken;

async fn act(
    manager: &PluginManager,
    root: &Path,
    agent: AgentId,
    operation: &str,
    request: PluginRequest,
) -> Result<PluginActivation, PluginError> {
    match manager
        .invoke(
            &agent,
            root,
            request,
            PluginInvocation {
                operation_id: operation,
                updates: None,
                cancellation: CancellationToken::new(),
            },
        )
        .await?
    {
        PluginOutcome::Activation(activation) => Ok(activation),
        _ => panic!("activation outcome"),
    }
}

fn package(root: &Path, skill: &str, body: &str, endpoint: Option<&str>) {
    fs::create_dir_all(root.join(format!("skills/{skill}"))).expect("skill directory");
    fs::write(
        root.join("plugin.json"),
        json!({"$schema":crate::plugins::inspect::PLUGIN_SCHEMA,"name":"same-name"}).to_string(),
    )
    .expect("manifest");
    fs::write(
        root.join(format!("skills/{skill}/SKILL.md")),
        format!("---\nname: {skill}\ndescription: {body}\n---\n{body}\n"),
    )
    .expect("skill");
    if let Some(endpoint) = endpoint {
        fs::write(root.join("mcp.json"),json!({"$schema":crate::plugins::inspect::MCP_SCHEMA,"mcpServers":{"main":{"type":"streamable-http","url":endpoint}}}).to_string()).expect("MCP manifest");
    }
}

async fn install(manager: &PluginManager, root: &Path, source: &str) -> String {
    let source = PluginSource::Package {
        source_path: source.into(),
    };
    let digest = inspect(manager, root, source.clone()).await;
    invoke(
        manager,
        root,
        PluginRequest::Install {
            source,
            expected_digest: digest.clone(),
        },
    )
    .await
    .expect("install exact revision");
    digest
}

async fn search(
    manager: &PluginManager,
    agent: AgentId,
    query: serde_json::Value,
) -> serde_json::Value {
    use renoa_agent::{ContentBlock, ToolCall, invoke_tool};
    let tool = crate::plugins::PluginSearchTool::new(agent, manager.clone(), true);
    let output = invoke_tool(
        Some(&tool),
        ToolCall {
            id: "search".to_owned(),
            name: "plugin_search".to_owned(),
            arguments: query,
            namespace: None,
            thought_signature: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("search");
    let ContentBlock::Text { text } = &output.content[0] else {
        panic!("text output");
    };
    serde_json::from_str(text).expect("search JSON")
}

#[tokio::test]
async fn equal_names_have_distinct_identities_and_collisions_never_replace_existing_skills() {
    let root = tempfile::tempdir().expect("fixture");
    let (manager, skills) = manager(root.path());
    package(&root.path().join("first"), "review", "FIRST", None);
    package(&root.path().join("second"), "write", "SECOND", None);
    package(&root.path().join("collision"), "review", "COLLISION", None);
    let agent = test_agent_id(1);
    let first = install(&manager, root.path(), "first").await;
    let second = install(&manager, root.path(), "second").await;
    let collision = install(&manager, root.path(), "collision").await;
    for (operation, digest) in [("first", &first), ("second", &second)] {
        act(
            &manager,
            root.path(),
            agent,
            operation,
            PluginRequest::Activate {
                package_digest: digest.clone(),
            },
        )
        .await
        .expect("separate selection");
    }
    let rejected = act(
        &manager,
        root.path(),
        agent,
        "collision",
        PluginRequest::Activate {
            package_digest: collision.clone(),
        },
    )
    .await
    .expect("component isolation");
    assert!(
        rejected
            .skills
            .as_ref()
            .expect("component report")
            .accepted()
            .is_empty()
    );
    assert_eq!(rejected.skills.as_ref().unwrap().rejected().len(), 1);
    let cards = search(&manager, agent, json!({"query":"same-name"})).await;
    let items = cards["items"].as_array().expect("cards");
    assert_eq!(items.len(), 3);
    for (digest, count) in [(&first, 1), (&second, 1), (&collision, 0)] {
        let card = items
            .iter()
            .find(|item| item["id"] == *digest)
            .expect("exact revision");
        assert_eq!(card["plugin_id"], *digest);
        assert_eq!(card["active_skills"], count);
    }
    let loaded = load_skill(&skills, agent, root.path(), SessionId::new());
    assert!(loaded.contains("FIRST"));
    assert!(!loaded.contains("COLLISION"));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn explicit_replacement_and_disable_preserve_pins_and_other_agents_across_restart() {
    let root = tempfile::tempdir().expect("fixture");
    let (manager, skills) = manager(root.path());
    package(&root.path().join("first"), "review", "FIRST", None);
    package(&root.path().join("next"), "review", "NEXT", None);
    let first = install(&manager, root.path(), "first").await;
    let next = install(&manager, root.path(), "next").await;
    let agent = test_agent_id(1);
    let other = test_agent_id(2);
    let session = SessionId::new();
    for id in [agent, other] {
        act(
            &manager,
            root.path(),
            id,
            "activate",
            PluginRequest::Activate {
                package_digest: first.clone(),
            },
        )
        .await
        .expect("activate");
    }
    load_skill(&skills, agent, root.path(), session);
    let replace = || PluginRequest::ReplacePlugin {
        plugin_id: first.clone(),
        package_digest: next.clone(),
        expected_digest: first.clone(),
    };
    let result = act(&manager, root.path(), agent, "replace", replace())
        .await
        .expect("explicit replacement");
    assert_eq!(result.plugin_id, first);
    assert_eq!(result.package_digest, next);
    assert!(
        crate::skills::runtime_context(&skills, session, None)
            .unwrap()
            .unwrap()
            .instructions
            .contains("FIRST")
    );
    assert!(load_skill(&skills, agent, root.path(), SessionId::new()).contains("NEXT"));
    assert!(load_skill(&skills, other, root.path(), SessionId::new()).contains("FIRST"));
    act(
        &manager,
        root.path(),
        agent,
        "disable",
        PluginRequest::Deactivate {
            plugin_id: first.clone(),
        },
    )
    .await
    .expect("disable");
    assert!(
        skills
            .summaries(&agent.to_string(), root.path())
            .expect("future discovery")
            .is_empty()
    );
    assert!(load_skill(&skills, agent, root.path(), SessionId::new()).is_empty());
    assert!(
        crate::skills::runtime_context(&skills, session, None)
            .unwrap()
            .unwrap()
            .instructions
            .contains("FIRST")
    );
    let replay = act(&manager, root.path(), agent, "replace", replace())
        .await
        .expect("lost reply replay");
    assert_eq!(replay.package_digest, next);
    assert!(
        manager
            .activations(&agent)
            .await
            .expect("read actual state")
            .iter()
            .all(|activation| !activation.enabled)
    );
    let database = root.path().join("host.sqlite3");
    let reopened = PluginManager::initialize(
        database.clone(),
        root.path().join("plugins"),
        McpCatalogStore::open(database.clone()).unwrap(),
        None,
        None,
        McpCredentialResolver::default(),
        SkillStore::initialize(database, root.path().join("skills"), None).unwrap(),
    )
    .unwrap();
    act(
        &reopened,
        root.path(),
        agent,
        "enable",
        PluginRequest::EnablePlugin {
            plugin_id: first.clone(),
        },
    )
    .await
    .expect("restore selected replacement after restart");
    let cards = search(&reopened, agent, json!({"query":"same-name"})).await;
    assert_eq!(cards["items"][0]["id"], next);
    assert_eq!(cards["items"][0]["plugin_id"], first);
    assert_eq!(cards["items"][0]["active_skills"], 1);
    assert!(matches!(
        act(
            &reopened,
            root.path(),
            agent,
            "replace",
            PluginRequest::Deactivate { plugin_id: first }
        )
        .await,
        Err(PluginError::Conflict(_))
    ));
}

#[tokio::test]
async fn concurrent_replacements_have_one_winner_and_reject_before_skill_publication() {
    let root = tempfile::tempdir().expect("fixture");
    let (manager, _) = manager(root.path());
    let mut digests = Vec::new();
    for (source, body) in [("first", "FIRST"), ("a", "A"), ("b", "B")] {
        package(&root.path().join(source), "review", body, None);
        digests.push(install(&manager, root.path(), source).await);
    }
    let agent = test_agent_id(1);
    act(
        &manager,
        root.path(),
        agent,
        "activate",
        PluginRequest::Activate {
            package_digest: digests[0].clone(),
        },
    )
    .await
    .unwrap();
    let request = |index: usize| PluginRequest::ReplacePlugin {
        plugin_id: digests[0].clone(),
        package_digest: digests[index].clone(),
        expected_digest: digests[0].clone(),
    };
    let (a, b) = tokio::join!(
        act(&manager, root.path(), agent, "a", request(1)),
        act(&manager, root.path(), agent, "b", request(2))
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert!(matches!(
        if a.is_ok() { b } else { a },
        Err(PluginError::Conflict(_))
    ));
    assert_eq!(
        fs::read_dir(root.path().join("skills")).unwrap().count(),
        2,
        "only the original and winning skill revisions are published"
    );
    assert_eq!(manager.activations(&agent).await.unwrap().len(), 1);
}

fn commit_mcp(manager: &PluginManager, agent: AgentId, digest: &str, connection: &str) {
    let catalog = manager.mcp_catalog();
    let endpoint = "https://service.example/mcp";
    let candidate = crate::mcp::McpConnectionCandidate::new(
        crate::plugins::manager::integration_id(digest, "main"),
        connection.to_owned(),
        endpoint.to_owned(),
        crate::mcp::McpRequestHeaders::default(),
        crate::mcp::McpConnectionAuth::None,
    )
    .unwrap();
    catalog
        .commit_connection(
            &agent.to_string(),
            &candidate,
            &crate::mcp::tests::snapshot(connection, endpoint, &["read"]),
            false,
        )
        .expect("real catalog commit");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn combined_plugin_gates_all_accounts_and_retired_intermediate_revisions() {
    let root = tempfile::tempdir().expect("fixture");
    let (manager, _) = manager(root.path());
    let agent = test_agent_id(1);
    let other = test_agent_id(2);
    let mut digests = Vec::new();
    for (source, body) in [("a", "A"), ("b", "B"), ("c", "C")] {
        package(
            &root.path().join(source),
            "review",
            body,
            Some("https://service.example/mcp"),
        );
        digests.push(install(&manager, root.path(), source).await);
    }
    act(
        &manager,
        root.path(),
        agent,
        "activate",
        PluginRequest::Activate {
            package_digest: digests[0].clone(),
        },
    )
    .await
    .unwrap();
    commit_mcp(&manager, agent, &digests[0], "personal");
    commit_mcp(&manager, agent, &digests[0], "work");
    commit_mcp(&manager, other, &digests[0], "personal");
    let catalog = manager.mcp_catalog();
    let reference = catalog.agent_tool_summaries(&agent.to_string()).unwrap()[0]
        .reference()
        .unwrap();
    act(
        &manager,
        root.path(),
        agent,
        "disable",
        PluginRequest::Deactivate {
            plugin_id: digests[0].clone(),
        },
    )
    .await
    .unwrap();
    assert!(
        catalog
            .agent_tool_summaries(&agent.to_string())
            .unwrap()
            .is_empty()
    );
    assert!(
        catalog
            .resolve_agent_tools(&agent.to_string(), &[reference])
            .is_err()
    );
    assert_eq!(
        catalog
            .agent_tool_summaries(&other.to_string())
            .unwrap()
            .len(),
        1
    );
    assert!(
        catalog
            .enable_agent_connection(&agent.to_string(), "work")
            .is_err()
    );
    act(
        &manager,
        root.path(),
        agent,
        "enable",
        PluginRequest::EnablePlugin {
            plugin_id: digests[0].clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        catalog
            .agent_tool_summaries(&agent.to_string())
            .unwrap()
            .len(),
        2
    );
    for (index, op) in [(1, "b"), (2, "c")] {
        act(
            &manager,
            root.path(),
            agent,
            op,
            PluginRequest::ReplacePlugin {
                plugin_id: digests[0].clone(),
                package_digest: digests[index].clone(),
                expected_digest: digests[index - 1].clone(),
            },
        )
        .await
        .unwrap();
        if index == 1 {
            commit_mcp(&manager, agent, &digests[1], "intermediate");
        }
    }
    assert!(
        catalog
            .agent_tool_summaries(&agent.to_string())
            .unwrap()
            .is_empty()
    );
    assert!(
        catalog
            .enable_agent_connection(&agent.to_string(), "intermediate")
            .is_err()
    );
    assert!(matches!(
        act(
            &manager,
            root.path(),
            agent,
            "resurrect",
            PluginRequest::Activate {
                package_digest: digests[1].clone()
            }
        )
        .await,
        Err(PluginError::Conflict(_))
    ));
    assert_eq!(manager.activations(&agent).await.unwrap().len(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn corrupt_plugin_can_be_disabled_and_missing_owners_fail_closed() {
    let root = tempfile::tempdir().expect("fixture");
    let (manager, _) = manager(root.path());
    let agent = test_agent_id(1);
    package(
        &root.path().join("plugin"),
        "review",
        "BODY",
        Some("https://service.example/mcp"),
    );
    let digest = install(&manager, root.path(), "plugin").await;
    commit_mcp(&manager, agent, &digest, "account");
    let connection = rusqlite::Connection::open(root.path().join("host.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE plugin_mcp_servers SET integration_id='wrong-owner' WHERE plugin_digest=?1",
            [&digest],
        )
        .unwrap();
    assert!(manager.load_local(&digest).await.is_err());
    assert!(
        manager
            .mcp_catalog()
            .agent_tool_summaries(&agent.to_string())
            .is_err()
    );
    connection
        .execute(
            "UPDATE plugin_mcp_servers SET integration_id=?2 WHERE plugin_digest=?1",
            rusqlite::params![
                digest,
                crate::plugins::manager::integration_id(&digest, "main")
            ],
        )
        .unwrap();
    let manifest = root
        .path()
        .join("plugins")
        .join(&digest)
        .join("plugin.json");
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&manifest, b"damaged manifest").unwrap();
    act(
        &manager,
        root.path(),
        agent,
        "disable",
        PluginRequest::Deactivate {
            plugin_id: digest.clone(),
        },
    )
    .await
    .expect("deactivation does not need content");
    assert!(
        manager
            .mcp_catalog()
            .agent_tool_summaries(&agent.to_string())
            .unwrap()
            .is_empty()
    );
    assert!(
        act(
            &manager,
            root.path(),
            agent,
            "enable",
            PluginRequest::EnablePlugin { plugin_id: digest }
        )
        .await
        .is_err()
    );
}

fn load_skill(skills: &SkillStore, agent: AgentId, root: &Path, session: SessionId) -> String {
    crate::skills::frozen_instructions(
        skills,
        &agent.to_string(),
        root,
        session,
        CommandId::new(),
        "review",
    )
    .expect("real skill runtime selection")
}

#[tokio::test]
async fn another_plugins_owner_cannot_bypass_deactivation() {
    let root = tempfile::tempdir().unwrap();
    let (manager, _) = manager(root.path());
    let agent = test_agent_id(1);
    let mut digests = Vec::new();
    for source in ["a", "b"] {
        package(
            &root.path().join(source),
            source,
            source,
            Some("https://service.example/mcp"),
        );
        let digest = install(&manager, root.path(), source).await;
        commit_mcp(&manager, agent, &digest, source);
        digests.push(digest);
    }
    act(
        &manager,
        root.path(),
        agent,
        "disable-a",
        PluginRequest::Deactivate {
            plugin_id: digests[0].clone(),
        },
    )
    .await
    .unwrap();
    let catalog = manager.mcp_catalog();
    let reference = catalog.agent_tool_summaries(&agent.to_string()).unwrap()[0]
        .reference()
        .unwrap();
    let connection = rusqlite::Connection::open(root.path().join("host.sqlite3")).unwrap();
    connection
        .execute(
            "UPDATE plugin_mcp_servers SET integration_id='temporary' WHERE plugin_digest=?1",
            [&digests[0]],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE plugin_mcp_servers SET integration_id=?2 WHERE plugin_digest=?1",
            rusqlite::params![
                digests[1],
                crate::plugins::manager::integration_id(&digests[0], "main")
            ],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE plugin_mcp_servers SET integration_id=?2 WHERE plugin_digest=?1",
            rusqlite::params![
                digests[0],
                crate::plugins::manager::integration_id(&digests[1], "main")
            ],
        )
        .unwrap();
    assert!(catalog.agent_tool_summaries(&agent.to_string()).is_err());
    assert!(
        catalog
            .resolve_agent_tools(&agent.to_string(), &[reference])
            .is_err()
    );
}

#[tokio::test]
async fn disabled_plugin_rejects_connect_and_authorize_before_oauth_effects() {
    let root = tempfile::tempdir().unwrap();
    let (manager, skills) = manager(root.path());
    let agent = test_agent_id(1);
    package(
        &root.path().join("plugin"),
        "review",
        "BODY",
        Some("https://service.example/mcp"),
    );
    let digest = install(&manager, root.path(), "plugin").await;
    let catalog = manager.mcp_catalog();
    let candidate = crate::mcp::McpConnectionCandidate::new(
        crate::plugins::manager::integration_id(&digest, "main"),
        "account".into(),
        "https://service.example/mcp".into(),
        crate::mcp::McpRequestHeaders::default(),
        crate::mcp::McpConnectionAuth::oauth(
            "account",
            "https://service.example/mcp",
            crate::mcp::McpOAuthRegistration::dynamic(),
        )
        .unwrap(),
    )
    .unwrap();
    catalog
        .commit_connection(
            &agent.to_string(),
            &candidate,
            &crate::mcp::tests::snapshot("account", "https://service.example/mcp", &["read"]),
            false,
        )
        .unwrap();
    act(
        &manager,
        root.path(),
        agent,
        "disable",
        PluginRequest::Deactivate {
            plugin_id: digest.clone(),
        },
    )
    .await
    .unwrap();
    let adapter = root.path().join("effect.mjs");
    fs::write(&adapter,"import {writeFileSync} from 'node:fs'; writeFileSync(new URL('oauth-effect',import.meta.url),'called'); process.exit(1);").unwrap();
    let manager = PluginManager::initialize(
        root.path().join("host.sqlite3"),
        root.path().join("plugins"),
        catalog,
        Some(adapter),
        None,
        McpCredentialResolver::default(),
        skills,
    )
    .unwrap();
    for request in [
        PluginRequest::Connect {
            package_digest: digest,
            server: "main".into(),
            connection: "new-account".into(),
            credential: Some(PluginAuthentication::OAuth {}),
            replace: false,
            restart: false,
            required_scope: None,
        },
        PluginRequest::Authorize {
            connection: "account".into(),
            restart: true,
            required_scope: None,
        },
    ] {
        let error = invoke(&manager, root.path(), request)
            .await
            .err()
            .expect("disabled plugin must reject before OAuth");
        assert!(matches!(
            error,
            PluginError::Mcp(crate::mcp::McpHostError::Conflict(_))
        ));
        assert!(!root.path().join("oauth-effect").exists());
        let connection = rusqlite::Connection::open(root.path().join("host.sqlite3")).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM mcp_oauth_flows", [], |r| r
                    .get::<_, u32>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM mcp_connections", [], |r| r
                    .get::<_, u32>(0))
                .unwrap(),
            1
        );
    }
}

#[tokio::test]
async fn failed_multi_skill_activation_removes_new_files_and_keeps_existing_targets() {
    let root = tempfile::tempdir().unwrap();
    let (manager, _) = manager(root.path());
    let source = root.path().join("plugin");
    for name in ["alpha", "beta"] {
        package(&source, name, name, None);
    }
    let digest = install(&manager, root.path(), "plugin").await;
    let alpha = crate::skills::package::capture(&source.join("skills/alpha"), None)
        .unwrap()
        .digest;
    let beta = crate::skills::package::capture(&source.join("skills/beta"), None)
        .unwrap()
        .digest;
    let existing = root.path().join("skills").join(&beta);
    fs::create_dir(&existing).unwrap();
    fs::write(existing.join("SKILL.md"), "damaged target").unwrap();
    for _ in 0..2 {
        assert!(
            act(
                &manager,
                root.path(),
                test_agent_id(1),
                "activate",
                PluginRequest::Activate {
                    package_digest: digest.clone()
                }
            )
            .await
            .is_err()
        );
        assert!(!root.path().join("skills").join(&alpha).exists());
        assert_eq!(
            fs::read(existing.join("SKILL.md")).unwrap(),
            b"damaged target"
        );
        let connection = rusqlite::Connection::open(root.path().join("host.sqlite3")).unwrap();
        for table in [
            "host_agent_plugins",
            "host_plugin_activation_operations",
            "skill_revisions",
            "agent_skill_bindings",
        ] {
            assert_eq!(
                connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, u32>(0))
                    .unwrap(),
                0
            );
        }
    }
    fs::remove_dir_all(&existing).unwrap();
    let connection = rusqlite::Connection::open(root.path().join("host.sqlite3")).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_activation_receipt BEFORE INSERT ON host_plugin_activation_operations BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END;").unwrap();
    assert!(
        act(
            &manager,
            root.path(),
            test_agent_id(1),
            "activate",
            PluginRequest::Activate {
                package_digest: digest
            }
        )
        .await
        .is_err()
    );
    assert!(!root.path().join("skills").join(alpha).exists());
    assert!(!existing.exists());
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM host_agent_plugins", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn retained_unreviewed_account_cannot_implicitly_activate_its_package() {
    let root = tempfile::tempdir().unwrap();
    let (manager, _) = manager(root.path());
    let rule = crate::plugins::PluginProviderFamily {
        family: "cloud".into(),
        origins: vec![
            "https://api.cloud.example".into(),
            "https://radar.cloud.example".into(),
        ],
    };
    crate::plugins::coherence::define(&root.path().join("host.sqlite3"), &rule).unwrap();
    let source = root.path().join("plugin");
    fs::create_dir(&source).unwrap();
    fs::write(
        source.join("plugin.json"),
        json!({"$schema":crate::plugins::inspect::PLUGIN_SCHEMA,"name":"cloud"}).to_string(),
    )
    .unwrap();
    fs::write(
        source.join("mcp.json"),
        json!({"$schema":crate::plugins::inspect::MCP_SCHEMA,"mcpServers":{
            "api":{"type":"streamable-http","url":"https://api.cloud.example/mcp"},
            "radar":{"type":"streamable-http","url":"https://radar.cloud.example/mcp"}
        }})
        .to_string(),
    )
    .unwrap();
    let digest = install(&manager, root.path(), "plugin").await;
    let catalog = manager.mcp_catalog();
    catalog
        .register_direct_connection(
            &crate::plugins::manager::integration_id(&digest, "api"),
            "retained",
            "https://api.cloud.example/mcp",
        )
        .unwrap();
    catalog
        .publish_catalog(&crate::mcp::tests::snapshot(
            "retained",
            "https://api.cloud.example/mcp",
            &["read"],
        ))
        .unwrap();
    // A migrated library keeps its content and accounts but has no new admission.
    let connection = rusqlite::Connection::open(root.path().join("host.sqlite3")).unwrap();
    connection.execute_batch("DELETE FROM host_plugin_admissions; DELETE FROM host_plugin_provider_origins; DELETE FROM host_plugin_provider_families;").unwrap();
    let agent = test_agent_id(1);
    assert!(
        catalog
            .enable_agent_connection(&agent.to_string(), "retained")
            .is_err()
    );
    assert!(
        act(
            &manager,
            root.path(),
            agent,
            "activate",
            PluginRequest::Activate {
                package_digest: digest.clone()
            }
        )
        .await
        .is_err()
    );
    assert!(
        catalog
            .agent_tool_summaries(&agent.to_string())
            .unwrap()
            .is_empty()
    );
    assert!(manager.activations(&agent).await.unwrap().is_empty());
    crate::plugins::coherence::define(&root.path().join("host.sqlite3"), &rule).unwrap();
    for source in [
        PluginSource::Installed {
            package_digest: digest.clone(),
        },
        PluginSource::Package {
            source_path: "plugin".into(),
        },
    ] {
        connection
            .execute("DELETE FROM host_plugin_admissions", [])
            .unwrap();
        invoke(
            &manager,
            root.path(),
            PluginRequest::Install {
                source,
                expected_digest: digest.clone(),
            },
        )
        .await
        .expect("explicit install re-admits verified retained revisions");
        catalog
            .enable_agent_connection(&agent.to_string(), "retained")
            .expect("successful install must allow selection");
        catalog
            .disable_agent_connection(&agent.to_string(), "retained")
            .unwrap();
    }
    act(
        &manager,
        root.path(),
        agent,
        "activate",
        PluginRequest::Activate {
            package_digest: digest,
        },
    )
    .await
    .expect("explicitly verify retained content after local review");
    catalog
        .enable_agent_connection(&agent.to_string(), "retained")
        .unwrap();
    assert_eq!(
        catalog
            .agent_tool_summaries(&agent.to_string())
            .unwrap()
            .len(),
        1
    );
}

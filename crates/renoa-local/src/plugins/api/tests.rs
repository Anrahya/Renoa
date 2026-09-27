use super::*;
use crate::{
    host::catalog,
    mcp::{McpCatalogStore, McpCredentialResolver},
    plugins::{
        PluginError, PluginManager,
        tests::{test_agent_id, test_skill_store},
    },
    skills::SkillStore,
};
use std::{fs, path::Path};

mod activation;
mod coherence;
mod github;
mod lifecycle;

fn manager(root: &Path) -> (PluginManager, SkillStore) {
    let database = root.join("host.sqlite3");
    catalog::initialize(&database).expect("catalog");
    let connection = rusqlite::Connection::open(&database).unwrap();
    for agent in [test_agent_id(1), test_agent_id(2)] {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM host_agents WHERE agent_id=?1)",
                [agent.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        if !exists {
            crate::test_agents::insert_agent(&database, &agent.to_string());
        }
    }
    let mcp = McpCatalogStore::open(database.clone()).expect("MCP store");
    let skills = test_skill_store(&database, root);
    let manager = PluginManager::initialize(
        database,
        root.join("plugins"),
        mcp,
        None,
        None,
        McpCredentialResolver::default(),
        skills.clone(),
    )
    .expect("manager");
    (manager, skills)
}
async fn invoke(
    manager: &PluginManager,
    root: &Path,
    request: PluginRequest,
) -> Result<PluginOutcome, PluginError> {
    manager
        .invoke(
            &test_agent_id(1),
            root,
            request,
            PluginInvocation {
                operation_id: "fixture-plugin-api",
                updates: None,
                cancellation: tokio_util::sync::CancellationToken::new(),
            },
        )
        .await
}
fn write_skill(root: &Path) {
    fs::create_dir_all(root.join("references")).expect("skill source");
    fs::write(root.join("SKILL.md"),"---\nname: review\ndescription: Review code.\nlicense: MIT\n---\nUse references/details.md.\n").expect("skill");
    fs::write(
        root.join("references/details.md"),
        "Inspect the implementation.",
    )
    .expect("reference");
}
async fn inspect(manager: &PluginManager, root: &Path, source: PluginSource) -> String {
    let PluginOutcome::Inspected(inspection) =
        invoke(manager, root, PluginRequest::Inspect { source })
            .await
            .expect("inspect")
    else {
        panic!("inspection outcome")
    };
    inspection.digest().to_owned()
}

#[tokio::test]
async fn a_standalone_skill_becomes_one_shared_plugin_and_replays_its_exact_revision() {
    let root = tempfile::tempdir().expect("fixture");
    let (manager, skills) = manager(root.path());
    let skill = root.path().join("local-skill");
    write_skill(&skill);
    let source = PluginSource::Skill {
        source_path: "local-skill".into(),
    };
    let digest = inspect(&manager, root.path(), source.clone()).await;
    assert!(
        manager
            .list()
            .await
            .expect("read-only inspection")
            .is_empty()
    );
    let request = || PluginRequest::Add {
        source: source.clone(),
        expected_digest: Some(digest.clone()),
        server: None,
        connection: None,
        credential: None,
        replace: false,
    };
    invoke(
        &manager,
        root.path(),
        PluginRequest::Install {
            source: source.clone(),
            expected_digest: digest.clone(),
        },
    )
    .await
    .expect("publish before activation");
    rusqlite::Connection::open(root.path().join("host.sqlite3"))
        .unwrap()
        .execute_batch("DELETE FROM installed_plugins;")
        .expect("publication acknowledgement lost before activation admission");
    let PluginOutcome::Added(added) = invoke(&manager, root.path(), request()).await.expect("add")
    else {
        panic!("add outcome")
    };
    assert_eq!(added.installed.digest(), digest);
    assert_eq!(added.skills.accepted(), ["review"]);
    assert!(
        fs::read_to_string(
            root.path()
                .join("plugins")
                .join(&digest)
                .join("skills/review/SKILL.md")
        )
        .expect("original skill metadata")
        .contains("license: MIT")
    );
    assert_eq!(
        fs::read(
            root.path()
                .join("plugins")
                .join(&digest)
                .join("skills/review/references/details.md")
        )
        .expect("owned reference"),
        b"Inspect the implementation."
    );
    fs::remove_dir_all(skill).expect("remove old source");
    invoke(&manager, root.path(), request())
        .await
        .expect("replay without source");
    assert_eq!(manager.list().await.expect("single installation").len(), 1);
    assert_eq!(
        skills
            .summaries(&test_agent_id(1).to_string(), root.path())
            .expect("enabled skills")
            .len(),
        1
    );
    let (manager, skills) = super::tests::manager(root.path());
    invoke(&manager, root.path(), request())
        .await
        .expect("replay after restart");
    assert_eq!(
        skills
            .summaries(&test_agent_id(1).to_string(), root.path())
            .expect("restart skill")[0]
            .name,
        "review"
    );
    assert!(
        skills
            .summaries(&test_agent_id(2).to_string(), root.path())
            .expect("other agent isolated")
            .is_empty()
    );
}

#[tokio::test]
async fn invalid_connection_selection_and_changed_content_leave_no_plugin_or_skill_binding() {
    let root = tempfile::tempdir().expect("fixture");
    let (manager, skills) = manager(root.path());
    write_skill(&root.path().join("skill"));
    let source = PluginSource::Skill {
        source_path: "skill".into(),
    };
    let digest = inspect(&manager, root.path(), source.clone()).await;
    let result = invoke(
        &manager,
        root.path(),
        PluginRequest::Add {
            source: source.clone(),
            expected_digest: Some(digest),
            server: Some("missing".to_owned()),
            connection: Some("account".to_owned()),
            credential: None,
            replace: false,
        },
    )
    .await;
    assert!(matches!(result, Err(PluginError::Invalid(_))));
    assert!(manager.list().await.expect("no admitted plugin").is_empty());
    assert_eq!(
        fs::read_dir(root.path().join("plugins"))
            .expect("no published tree")
            .count(),
        0
    );
    assert!(
        skills
            .summaries(&test_agent_id(1).to_string(), root.path())
            .expect("no skills")
            .is_empty()
    );
    let digest = inspect(&manager, root.path(), source.clone()).await;
    fs::write(root.path().join("skill/references/details.md"), "Changed.")
        .expect("mutate resource");
    assert!(matches!(
        invoke(
            &manager,
            root.path(),
            PluginRequest::Install {
                source,
                expected_digest: digest
            }
        )
        .await,
        Err(PluginError::Conflict(_))
    ));
    assert_eq!(
        fs::read_dir(root.path().join("plugins"))
            .expect("no published tree")
            .count(),
        0
    );
}

#[cfg(unix)]
#[tokio::test]
async fn denied_source_entries_fail_identically_on_first_attempt_and_retry_without_residue() {
    let root = tempfile::tempdir().expect("fixture");
    let (manager, _) = manager(root.path());
    let source = root.path().join("source");
    fs::create_dir(&source).expect("source");
    fs::write(
        source.join("plugin.json"),
        format!(
            r#"{{"$schema":"{}","name":"test"}}"#,
            crate::plugins::inspect::PLUGIN_SCHEMA
        ),
    )
    .expect("manifest");
    for wrong_kind in ["symlink", "directory"] {
        if wrong_kind == "symlink" {
            std::os::unix::fs::symlink("/outside", source.join("unrelated")).expect("symlink");
        } else {
            fs::remove_file(source.join("unrelated")).expect("remove symlink");
            fs::create_dir(source.join("mcp.json")).expect("wrong-kind MCP");
        }
        let locator = PluginSource::Package {
            source_path: "source".into(),
        };
        let digest = inspect(&manager, root.path(), locator.clone()).await;
        for _ in 0..2 {
            assert!(matches!(
                invoke(
                    &manager,
                    root.path(),
                    PluginRequest::Install {
                        source: locator.clone(),
                        expected_digest: digest.clone()
                    }
                )
                .await,
                Err(PluginError::Invalid(_))
            ));
            assert!(manager.list().await.expect("no row").is_empty());
            assert_eq!(
                fs::read_dir(root.path().join("plugins"))
                    .expect("no directory")
                    .count(),
                0
            );
        }
    }
}

#[tokio::test]
async fn local_intake_admits_content_before_reporting_shared_reconciliation_failure() {
    for add in [false, true] {
        let root = tempfile::tempdir().expect("fixture");
        let (mut manager, skills) = manager(root.path());
        write_skill(&root.path().join("skill"));
        let source = PluginSource::Skill {
            source_path: "skill".into(),
        };
        let digest = inspect(&manager, root.path(), source.clone()).await;
        manager = manager.with_shared_registry(Some(
            crate::shared_registry::SharedPluginRegistry::new(
                "http://127.0.0.1:9",
                root.path().join("host.sqlite3"),
                root.path(),
            )
            .expect("unreachable registry"),
        ));
        let request = || {
            if add {
                PluginRequest::Add {
                    source: source.clone(),
                    expected_digest: Some(digest.clone()),
                    server: None,
                    connection: None,
                    credential: None,
                    replace: false,
                }
            } else {
                PluginRequest::Install {
                    source: source.clone(),
                    expected_digest: digest.clone(),
                }
            }
        };
        for attempt in 0..2 {
            assert!(matches!(
                invoke(&manager, root.path(), request()).await,
                Err(PluginError::Unavailable(message)) if message.contains("is installed locally")
            ));
            assert_eq!(
                manager
                    .store
                    .load(&digest)
                    .expect("retained exact content")
                    .digest(),
                digest
            );
            assert_eq!(
                fs::read_dir(root.path().join("plugins"))
                    .expect("one revision")
                    .count(),
                1
            );
            assert!(
                skills
                    .summaries(&test_agent_id(1).to_string(), root.path())
                    .expect("no incomplete activation")
                    .is_empty()
            );
            if attempt == 0 {
                fs::remove_dir_all(root.path().join("skill")).expect("remove mutable source");
            }
        }
    }
}

#[tokio::test]
async fn every_install_retry_still_reconciles_the_shared_library() {
    let root = tempfile::tempdir().expect("fixture");
    let (mut manager, _) = manager(root.path());
    write_skill(&root.path().join("skill"));
    let source = PluginSource::Skill {
        source_path: "skill".into(),
    };
    let digest = inspect(&manager, root.path(), source.clone()).await;
    invoke(
        &manager,
        root.path(),
        PluginRequest::Install {
            source: source.clone(),
            expected_digest: digest.clone(),
        },
    )
    .await
    .expect("publish before the shared registry becomes unavailable");
    manager = manager.with_shared_registry(Some(
        crate::shared_registry::SharedPluginRegistry::new(
            "http://127.0.0.1:9",
            root.path().join("host.sqlite3"),
            root.path(),
        )
        .expect("unreachable registry"),
    ));
    for source in [
        source.clone(),
        source,
        PluginSource::Installed {
            package_digest: digest.clone(),
        },
    ] {
        let result = invoke(
            &manager,
            root.path(),
            PluginRequest::Install {
                source,
                expected_digest: digest.clone(),
            },
        )
        .await;
        assert!(
            matches!(result, Err(PluginError::Unavailable(_))),
            "a local publication cannot acknowledge an unfinished shared reconciliation"
        );
    }
}

#[test]
fn schemas_come_from_the_same_closed_action_and_source_types() {
    let complete = plugin_api_schema();
    let projected = manage_tool_spec("plugin_manage").input_schema;
    let variants = complete["oneOf"]
        .as_array()
        .expect("discriminated API contract");
    let actions = projected["properties"]["action"]["enum"]
        .as_array()
        .expect("model selector");
    assert_eq!(variants.len(), actions.len());
    for variant in variants {
        let action = variant["properties"]["action"]["const"]
            .as_str()
            .expect("exact action");
        assert!(actions.iter().any(|value| value == action));
        assert_eq!(variant["additionalProperties"], false);
    }
    for request in [
        serde_json::json!({"action":"add","source":{"kind":"skill","source_path":"review"}}),
        serde_json::json!({"action":"inspect","source":{"kind":"github","repository":"https://github.com/owner/repo","commit":"a".repeat(40)}}),
        serde_json::json!({"action":"install","source":{"kind":"package","source_path":"review"},"expected_digest":"a".repeat(64)}),
        serde_json::json!({"action":"connect","package_digest":"a".repeat(64),"server":"api","connection":"account"}),
        serde_json::json!({"action":"authorize","connection":"account"}),
        serde_json::json!({"action":"disconnect","connection":"account"}),
        serde_json::json!({"action":"enable","connection":"account"}),
    ] {
        serde_json::from_value::<PluginRequest>(request.clone()).expect("valid complete request");
        let variant = variants
            .iter()
            .find(|variant| variant["properties"]["action"]["const"] == request["action"])
            .expect("action schema");
        for required in variant["required"].as_array().expect("required fields") {
            let mut missing = request.clone();
            missing
                .as_object_mut()
                .expect("request")
                .remove(required.as_str().expect("field"));
            assert!(
                serde_json::from_value::<PluginRequest>(missing).is_err(),
                "missing {required} in {request}"
            );
        }
    }
    assert!(
        serde_json::from_value::<PluginRequest>(
            serde_json::json!({"action":"enable","connection":"shared","tools":["bash"]})
        )
        .is_err()
    );
    assert!(serde_json::from_value::<PluginRequest>(serde_json::json!({"action":"add","source":{"kind":"skill","source_path":"skill","tools":["bash"]}})).is_err());
    assert!(
        serde_json::to_vec(&projected).expect("schema").len() < 16 * 1024,
        "fixed model schema remains bounded"
    );
}

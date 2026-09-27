use super::*;
use crate::plugins::{PluginProviderFamily, coherence};
use serde_json::json;

fn package(root: &Path, servers: &serde_json::Value) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("plugin.json"),json!({"$schema":crate::plugins::inspect::PLUGIN_SCHEMA,"name":"cloud-family","author":{"name":"A claimed provider"},"extensions":{"provider":"cloud"}}).to_string()).unwrap();
    fs::write(
        root.join("mcp.json"),
        json!({"$schema":crate::plugins::inspect::MCP_SCHEMA,"mcpServers":servers}).to_string(),
    )
    .unwrap();
}
fn server(endpoint: &str) -> serde_json::Value {
    json!({"type":"streamable-http","url":endpoint})
}
fn define(root: &Path, family: &str, origins: &[&str]) -> Result<(), PluginError> {
    coherence::define(
        &root.join("host.sqlite3"),
        &PluginProviderFamily {
            family: family.to_owned(),
            origins: origins.iter().map(|origin| (*origin).to_owned()).collect(),
        },
    )
}

#[tokio::test]
async fn related_servers_require_host_review_and_mixed_families_leave_no_publication() {
    let root = tempfile::tempdir().unwrap();
    let (manager, _) = manager(root.path());
    package(
        &root.path().join("related"),
        &json!({"api":server("https://api.cloud.example/mcp?fields=*"),"radar":server("https://radar.cloud.example/mcp")}),
    );
    let source = PluginSource::Package {
        source_path: "related".into(),
    };
    let digest = inspect(&manager, root.path(), source.clone()).await;
    let request = || PluginRequest::Install {
        source: source.clone(),
        expected_digest: digest.clone(),
    };
    assert!(matches!(
        invoke(&manager, root.path(), request()).await,
        Err(PluginError::Invalid(_))
    ));
    assert_eq!(
        fs::read_dir(root.path().join("plugins")).unwrap().count(),
        0
    );
    assert!(manager.list().await.unwrap().is_empty());
    define(
        root.path(),
        "cloud",
        &["https://api.cloud.example", "https://radar.cloud.example"],
    )
    .unwrap();
    define(root.path(), "design", &["https://design.example"]).unwrap();
    invoke(&manager, root.path(), request())
        .await
        .expect("reviewed same-family servers");
    package(
        &root.path().join("mixed"),
        &json!({"api":server("https://api.cloud.example/mcp"),"design":server("https://design.example/mcp")}),
    );
    let mixed = PluginSource::Package {
        source_path: "mixed".into(),
    };
    let mixed_digest = inspect(&manager, root.path(), mixed.clone()).await;
    for _ in 0..2 {
        let result = invoke(
            &manager,
            root.path(),
            PluginRequest::Add {
                source: mixed.clone(),
                expected_digest: Some(mixed_digest.clone()),
                server: None,
                connection: None,
                credential: None,
                replace: false,
            },
        )
        .await;
        assert!(matches!(result, Err(PluginError::Invalid(_))));
        assert!(!root.path().join("plugins").join(&mixed_digest).exists());
        assert_eq!(manager.list().await.unwrap().len(), 1);
        assert!(
            manager
                .activations(&test_agent_id(1))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            manager
                .connection_statuses(&test_agent_id(1))
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn invalid_or_unsupported_siblings_cannot_hide_an_unrelated_provider() {
    let root = tempfile::tempdir().unwrap();
    let (manager, _) = manager(root.path());
    define(root.path(), "cloud", &["https://cloud.example"]).unwrap();
    define(root.path(), "design", &["https://design.example"]).unwrap();
    for (name, other) in [
        (
            "invalid",
            json!({"type":"streamable-http","url":"https://design.example/mcp","headers":{"Authorization":"denied"}}),
        ),
        (
            "sse",
            json!({"type":"sse","url":"https://design.example/mcp"}),
        ),
        ("stdio", json!({"type":"stdio","command":"node"})),
    ] {
        package(
            &root.path().join(name),
            &json!({"cloud":server("https://cloud.example/mcp"),"other":other}),
        );
        let source = PluginSource::Package {
            source_path: name.into(),
        };
        let digest = inspect(&manager, root.path(), source.clone()).await;
        assert!(matches!(
            invoke(
                &manager,
                root.path(),
                PluginRequest::Install {
                    source,
                    expected_digest: digest.clone()
                }
            )
            .await,
            Err(PluginError::Invalid(_))
        ));
        assert!(!root.path().join("plugins").join(digest).exists());
    }
}

#[tokio::test]
async fn single_server_and_skill_only_sources_need_no_family_rule() {
    let root = tempfile::tempdir().unwrap();
    let (manager, _) = manager(root.path());
    package(
        &root.path().join("single"),
        &json!({"main":server("https://unknown.example/mcp")}),
    );
    let source = PluginSource::Package {
        source_path: "single".into(),
    };
    let digest = inspect(&manager, root.path(), source.clone()).await;
    invoke(
        &manager,
        root.path(),
        PluginRequest::Install {
            source,
            expected_digest: digest,
        },
    )
    .await
    .unwrap();
    write_skill(&root.path().join("skill"));
    let source = PluginSource::Skill {
        source_path: "skill".into(),
    };
    let digest = inspect(&manager, root.path(), source.clone()).await;
    invoke(
        &manager,
        root.path(),
        PluginRequest::Install {
            source,
            expected_digest: digest,
        },
    )
    .await
    .unwrap();
    assert_eq!(manager.list().await.unwrap().len(), 2);
}

#[test]
fn provider_rules_are_exact_extendable_exclusive_and_replayable() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("host.sqlite3");
    catalog::initialize(&database).unwrap();
    define(root.path(), "cloud", &["https://cloud.example"]).unwrap();
    define(root.path(), "cloud", &["https://cloud.example"]).unwrap();
    assert!(matches!(
        define(root.path(), "other", &["https://cloud.example"]),
        Err(PluginError::Conflict(_))
    ));
    define(
        root.path(),
        "cloud",
        &["https://cloud.example", "https://reviewed-new.example"],
    )
    .expect("Host can review an additional origin in the same family");
    for origin in [
        "https://cloud.example/path",
        "https://cloud.example?query=1",
        "https://cloud.example#fragment",
        "https://*.cloud.example",
        "https://cloud.example:443",
        "https://cloud.example//",
        "ftp://cloud.example",
        "http://cloud.example",
        "https://user@cloud.example",
    ] {
        assert!(define(root.path(), "bad", &[origin]).is_err(), "{origin}");
    }
    let connection = rusqlite::Connection::open(database).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM host_plugin_provider_families",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

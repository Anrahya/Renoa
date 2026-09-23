use std::fs;
use std::path::Path;

use rusqlite::Connection;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::super::{HostInitialization, reset_host_data_root};
use crate::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentPresetId, LocalHost, ModelProvider,
    RoutineMutation, RoutineSchedule, RoutineSpec,
    presets::{ARCEE_PRESET_ID, SPECIALIST_PRESET_ID},
};

const RETAINED_INTEGRATION: &str = "retained.integration";
const REQUEST_ID: &str = "00000000-0000-0000-0000-000000000001";

/// A managed root that is a symbolic link must be refused rather than followed,
/// and the refusal must come before any durable agent state is deleted.
#[tokio::test]
async fn a_symlinked_managed_root_is_refused() {
    let (directory, host) = fixture();
    let root = directory.path();
    let agent = seed_agent(&host).await;
    drop(host);
    let elsewhere = tempdir().expect("escape target");
    fs::write(elsewhere.path().join("kept.txt"), "keep\n").expect("external file");
    let sessions = root.join("data/sessions");
    fs::remove_dir(&sessions).expect("replace the session root");
    std::os::unix::fs::symlink(elsewhere.path(), &sessions).expect("link sessions");

    let error = reset_host_data_root(&root.join("data"))
        .expect_err("a symlinked managed root must be refused");
    assert!(
        error.to_string().contains("symbolic link"),
        "unexpected error: {error}"
    );
    assert_eq!(
        fs::read_to_string(elsewhere.path().join("kept.txt")).expect("external file survives"),
        "keep\n"
    );
    assert_eq!(
        count(&database(root), "host_agents"),
        1,
        "a refused reset must not delete rows"
    );
    assert!(
        root.join("data/agents").join(agent.to_string()).exists(),
        "a refused reset must not delete the agent's documents"
    );
}

/// A managed root that exists as a file is refused before the reset deletes
/// anything, instead of being silently skipped.
#[tokio::test]
async fn a_managed_root_that_is_not_a_directory_is_refused() {
    let (directory, host) = fixture();
    let root = directory.path();
    let agent = seed_agent(&host).await;
    drop(host);
    let sessions = root.join("data/sessions");
    fs::remove_dir(&sessions).expect("replace the session root");
    fs::write(&sessions, "not a directory\n").expect("file at the managed root");

    let error = reset_host_data_root(&root.join("data"))
        .expect_err("a managed root that is not a directory must be refused");
    assert!(
        error.to_string().contains("managed root"),
        "unexpected error: {error}"
    );
    assert_eq!(
        count(&database(root), "host_agents"),
        1,
        "a refused reset must not delete rows"
    );
    assert!(
        root.join("data/agents").join(agent.to_string()).exists(),
        "a refused reset must not delete the agent's documents"
    );
}

#[tokio::test]
async fn a_catalog_failure_rolls_back_cutover_and_agent_row_clearing() {
    let (directory, host) = fixture();
    let root = directory.path();
    let agent = seed_agent(&host).await;
    drop(host);
    Connection::open(database(root))
        .expect("open Host catalog")
        .execute_batch(
            "DROP TABLE host_agent_creations;
             CREATE TABLE host_agent_creations (
                operation_id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL REFERENCES host_agents(agent_id),
                request_json TEXT NOT NULL CHECK (json_valid(request_json))
             ) STRICT;
             UPDATE host_metadata SET schema_version = 27 WHERE singleton = 1;
             PRAGMA user_version = 27;",
        )
        .expect("construct schema-twenty-seven fixture");
    super::super::catalog::fail_next_clear_before_commit();

    let error = reset_host_data_root(&root.join("data"))
        .expect_err("the injected catalog failure must abort the reset");

    assert!(error.to_string().contains("injected catalog failure"));
    assert_eq!(count(&database(root), "host_agents"), 1);
    let catalog = Connection::open(database(root)).expect("open rolled-back catalog");
    assert_eq!(
        catalog
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .expect("read rolled-back schema version"),
        27
    );
    assert_eq!(
        catalog
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('host_agent_creations')
                 WHERE name = 'result_json'",
                [],
                |row| row.get::<_, u32>(0),
            )
            .expect("read rolled-back creation receipt columns"),
        0,
        "the failed reset must roll the schema cutover back too"
    );
    assert!(
        root.join("data/agents").join(agent.to_string()).exists(),
        "a rolled-back catalog reset must not reach filesystem clearing"
    );
}

/// Creates one durable agent, whose rows and documents a refused reset must keep.
async fn seed_agent(host: &LocalHost) -> crate::AgentId {
    host.create_agent(
        AgentCreator::System {
            component: "reset-refusal-test".to_owned(),
        },
        AgentCreationOrigin::Provisioning,
        AgentCreateRequest::new(
            Uuid::new_v4(),
            AgentPresetId::new(ARCEE_PRESET_ID).expect("preset id"),
            "Operator",
        ),
        CancellationToken::new(),
    )
    .await
    .expect("agent")
    .id
}

fn open_host(root: &Path) -> LocalHost {
    LocalHost::assemble(HostInitialization {
        data_directory: root.join("data"),
        bridge: root.join("model.mjs"),
        providers: vec![ModelProvider::Xai],
        initial_provider: ModelProvider::Xai,
        initial_model: "fixture".to_owned(),
        initial_reasoning: None,
        credential_store: root.join("auth.sqlite"),
        mcp_adapter: None,
        mcp_registry_adapter: None,
        shared_plugin_registry: None,
        global_skill_source: None,
        oauth_relay: None,
        code_mode: None,
    })
    .expect("Host")
}

fn fixture() -> (tempfile::TempDir, LocalHost) {
    let directory = tempdir().expect("fixture");
    let root = directory.path();
    fs::write(root.join("model.mjs"), "// fixture\n").expect("model");
    fs::write(root.join("auth.sqlite"), "").expect("auth boundary");
    let host = open_host(root);
    (directory, host)
}

/// Registers one retained MCP integration, the shared state a reset must keep.
async fn register_retained_connection(host: &LocalHost) {
    host.register_direct_mcp_connection(
        RETAINED_INTEGRATION,
        "retained",
        "https://example.com/mcp",
    )
    .await
    .expect("retained connection");
}

fn count(path: &Path, table: &str) -> i64 {
    let connection = Connection::open(path).expect("open Host catalog");
    connection
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("count rows")
}

fn database(root: &Path) -> std::path::PathBuf {
    root.join("data").join("host.sqlite3")
}

/// Writes one review binding chain for an agent, so the reset's delete set
/// spans the tables that reference the agent root.
fn seed_review_records(path: &Path, agent: crate::AgentId) {
    let connection = Connection::open(path).expect("open Host catalog");
    connection
        .execute_batch(&format!(
            "INSERT INTO host_review_repositories(repository_id, agent_id, record_json)
             VALUES (7, '{agent}', '{{}}');
             INSERT INTO host_review_requests(id, repository_id, repository_json, pull_number,
                base_sha, head_sha, admitted_at_ms)
             VALUES ('{REQUEST_ID}', 7, '{{}}', 1, 'a', 'b', 0);
             INSERT INTO host_review_runs(request_id, terminal, record_json)
             VALUES ('{REQUEST_ID}', 0, '{{}}');"
        ))
        .expect("review binding fixture");
}

#[tokio::test]
async fn a_reset_removes_agent_state_and_keeps_shared_state() {
    let (directory, host) = fixture();
    let root = directory.path();
    register_retained_connection(&host).await;
    let agent = host
        .create_agent(
            AgentCreator::System {
                component: "reset-test".to_owned(),
            },
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::new(
                Uuid::new_v4(),
                AgentPresetId::new(ARCEE_PRESET_ID).expect("preset id"),
                "Operator",
            ),
            CancellationToken::new(),
        )
        .await
        .expect("agent");
    host.manage_routine(
        agent.id,
        Uuid::new_v4(),
        RoutineMutation::Create {
            spec: RoutineSpec {
                agent_id: agent.id,
                name: "Digest".to_owned(),
                prompt: "Write the digest.".to_owned(),
                schedule: RoutineSchedule::Interval { hours: 12 },
                enabled: true,
            },
        },
        0,
        CancellationToken::new(),
    )
    .await
    .expect("routine");
    seed_review_records(&database(root), agent.id);
    let review_workspace = root.join("data/review-workspaces").join(REQUEST_ID);
    fs::create_dir_all(&review_workspace).expect("review workspace");
    fs::write(review_workspace.join("checkout.txt"), "discarded\n").expect("checkout file");
    let execution = root.join("data/github-executions").join(REQUEST_ID);
    fs::create_dir_all(&execution).expect("execution directory");
    fs::write(execution.join("app.jwt"), "discarded\n").expect("execution file");
    let workspace = host.agent_workspace(agent.id).await.expect("workspace");
    fs::write(workspace.join("notes.md"), "kept\n").expect("workspace file");
    let sessions = root.join("data/sessions");
    fs::create_dir_all(sessions.join("session-one")).expect("session directory");
    fs::write(sessions.join("session-one/manifest.json"), "{}\n").expect("session file");

    let report = reset_host_data_root(&root.join("data")).expect("reset");

    assert!(report.total_rows() >= 6, "{report:?}");
    assert_eq!(report.removed_sessions, 1);
    assert_eq!(report.removed_review_directories, 2);
    assert_eq!(report.removed_document_roots, 1);
    assert_eq!(report.preserved_workspaces, ["agent-workspaces"]);
    assert!(!root.join("data/agents").join(agent.id.to_string()).exists());
    assert!(!review_workspace.exists());
    assert!(!execution.exists());
    let path = database(root);
    assert_eq!(count(&path, "host_agents"), 0);
    assert_eq!(count(&path, "host_agent_tool_selections"), 0);
    assert_eq!(count(&path, "host_agent_creations"), 0);
    assert_eq!(count(&path, "mcp_integrations"), 1);
    assert_eq!(count(&path, "host_identity"), 1);
    assert!(!sessions.join("session-one").exists());
    assert_eq!(
        fs::read_to_string(workspace.join("notes.md")).expect("kept file"),
        "kept\n"
    );

    let second = reset_host_data_root(&root.join("data")).expect("repeat reset");
    assert_eq!(second.total_rows(), 0);
    assert_eq!(second.removed_sessions, 0);
    assert_eq!(second.removed_review_directories, 0);
    assert_eq!(second.removed_document_roots, 0);
    assert_eq!(count(&path, "mcp_integrations"), 1);
}

#[tokio::test]
async fn a_data_root_from_an_earlier_runtime_migrates_onto_the_canonical_tables() {
    let (directory, host) = fixture();
    let root = directory.path();
    let path = database(root);
    drop(host);
    {
        let connection = Connection::open(&path).expect("open catalog");
        connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 CREATE TABLE host_bots (
                    bot_id TEXT PRIMARY KEY,
                    created_by TEXT NOT NULL,
                    recipe_json TEXT NOT NULL
                 ) STRICT;
                 CREATE TABLE profile_mcp_connections (
                    profile_id TEXT NOT NULL,
                    connection_id TEXT NOT NULL,
                    PRIMARY KEY (profile_id, connection_id)
                 ) STRICT;
                 CREATE TABLE profile_skill_bindings (
                    profile_id TEXT NOT NULL,
                    source_id TEXT NOT NULL,
                    skill_name TEXT NOT NULL
                 ) STRICT;
                 INSERT INTO host_bots VALUES ('legacy-bot', 'legacy-owner', '{}');
                 INSERT INTO mcp_integrations(integration_id, kind, endpoint, request_headers_json)
                 VALUES ('retained.integration', 'direct_streamable_http', 'https://example.com/mcp', '{}');
                 DROP TABLE host_agent_tool_selections;
                 DROP TABLE host_agent_creations;
                 DROP TABLE host_agents;
                 UPDATE host_metadata SET schema_version = 25 WHERE singleton = 1;
                 PRAGMA user_version = 25;",
            )
            .expect("earlier runtime fixture");
    }

    let refused = crate::host::catalog::initialize(&database(root));
    assert!(
        matches!(&refused, Err(crate::HostCatalogError::Invalid(message)) if message.contains("reset")),
        "an earlier data root must be refused until it is reset: {refused:?}"
    );

    let predecessor_documents = root.join("data/profiles").join(ARCEE_PRESET_ID);
    fs::create_dir_all(&predecessor_documents).expect("predecessor document root");
    fs::write(predecessor_documents.join("SOUL.md"), "predecessor\n")
        .expect("predecessor document");

    let report = reset_host_data_root(&root.join("data")).expect("cutover reset");
    assert_eq!(report.total_rows(), 0, "{report:?}");
    assert_eq!(report.removed_document_roots, 1, "{report:?}");
    assert!(
        !predecessor_documents.exists(),
        "the predecessor document root must not survive the cutover"
    );
    let migrated = open_host(root);
    migrated
        .agent_definition(crate::AgentId::from_uuid(Uuid::new_v4()))
        .await
        .expect("canonical tables are queryable");
    let path = database(root);
    assert_eq!(count(&path, "host_agents"), 0);
    assert_eq!(count(&path, "mcp_integrations"), 1);
    assert_eq!(count(&path, "host_identity"), 1);
    for retired in [
        "host_bots",
        "profile_mcp_connections",
        "profile_skill_bindings",
    ] {
        let connection = Connection::open(&path).expect("open catalog");
        let present: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [retired],
                |row| row.get(0),
            )
            .expect("table lookup");
        assert_eq!(present, 0, "retired table {retired} survived migration");
    }

    let agent = migrated
        .create_agent(
            AgentCreator::System {
                component: "post-migration".to_owned(),
            },
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::new(
                Uuid::new_v4(),
                AgentPresetId::new(SPECIALIST_PRESET_ID).expect("preset id"),
                "Fresh",
            )
            .with_instructions("Run after the cutover."),
            CancellationToken::new(),
        )
        .await
        .expect("post-migration creation");
    assert_eq!(count(&path, "host_agents"), 1);
    assert_eq!(
        migrated.agent_definition(agent.id).await.expect("read"),
        Some(agent)
    );
}

/// Every table in the canonical catalog is either agent-owned or shared Host
/// state. Deferred foreign keys only fail a reset when an omitted table leaves a
/// row referencing a deleted parent, so this classification — not the commit —
/// is what keeps the delete set complete: a new table fails here until it is
/// placed on one side.
#[test]
fn every_catalog_table_is_classified_agent_owned_or_shared() {
    // Shared Host state, which a reset must keep.
    const SHARED: &[&str] = &[
        "host_identity",
        "host_metadata",
        "installed_plugins",
        "mcp_catalogs",
        "mcp_connections",
        "mcp_integrations",
        "mcp_oauth_flows",
        "mcp_oauth_receipts",
        "mcp_rejected_tools",
        "mcp_tools",
        "plugin_mcp_servers",
        "shared_plugin_registry_state",
        "skill_revisions",
    ];
    let (directory, _host) = fixture();
    let connection = Connection::open(database(directory.path())).expect("open Host catalog");
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .expect("prepare catalog query");
    let tables: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .expect("query catalog tables")
        .collect::<Result<_, _>>()
        .expect("read catalog tables");
    assert!(
        tables.len() > super::AGENT_OWNED_TABLES.len(),
        "the catalog must expose both classified sets: {tables:?}"
    );
    for table in &tables {
        assert!(
            super::AGENT_OWNED_TABLES.contains(&table.as_str()) || SHARED.contains(&table.as_str()),
            "`{table}` is unclassified: add it to AGENT_OWNED_TABLES in reset.rs or to SHARED here"
        );
    }
}

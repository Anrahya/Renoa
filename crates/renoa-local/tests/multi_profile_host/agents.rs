use super::*;
use renoa_kernel::AgentId;
use renoa_local::AgentRecord;

struct Fixture {
    directory: tempfile::TempDir,
    workspace: std::path::PathBuf,
    bridge: std::path::PathBuf,
    auth: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempdir().expect("Host fixture");
        let workspace = directory.path().join("workspace");
        let bridge = directory.path().join("model.mjs");
        let auth = directory.path().join("auth.sqlite");
        fs::create_dir(&workspace).expect("workspace");
        fs::write(&bridge, MODEL_BRIDGE).expect("model fixture");
        fs::write(&auth, "").expect("auth fixture");
        Self {
            directory,
            workspace,
            bridge,
            auth,
        }
    }

    fn host(&self) -> LocalHost {
        local_host(
            &self.directory.path().join("data"),
            &self.bridge,
            &self.auth,
            true,
        )
    }

    fn record(name: &str, created_by: Option<AgentId>) -> AgentRecord {
        AgentRecord {
            id: AgentId::new(),
            profile: AgentProfileId::new(RELAY_PROFILE_ID).expect("profile"),
            name: name.to_owned(),
            created_by,
        }
    }
}

#[tokio::test]
async fn durable_roster_and_creator_relationship_survive_restart_without_model_dependencies() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let host_id = host.host_id().await.expect("Host identity");
    let parent = host
        .ensure_agent(Fixture::record("Operator", None))
        .await
        .expect("parent");
    let child = host
        .ensure_agent(Fixture::record("News", Some(parent.id)))
        .await
        .expect("child");
    assert_eq!(
        host.ensure_agent(child.clone())
            .await
            .expect("retry creation"),
        child
    );
    let mut changed = child.clone();
    changed.name = "Other".to_owned();
    assert!(
        matches!(host.ensure_agent(changed).await, Err(LocalHostError::AgentConflict(id)) if id == child.id)
    );
    drop(host);
    fs::remove_file(&fixture.bridge).expect("disable model dependency");
    let restarted = fixture.host();
    assert_eq!(restarted.host_id().await.expect("same host"), host_id);
    assert_eq!(
        restarted.agent(child.id).await.expect("inspect child"),
        Some(child.clone())
    );
    let agents = restarted.list_agents().await.expect("roster without model");
    assert_eq!(agents.len(), 2);
    assert!(agents.contains(&parent) && agents.contains(&child));
    assert_eq!(
        restarted
            .ensure_agent(child.clone())
            .await
            .expect("retry after restart"),
        child
    );
    let unrelated = Fixture::new();
    assert_ne!(
        unrelated
            .host()
            .host_id()
            .await
            .expect("different data root"),
        host_id
    );
}

#[tokio::test]
async fn competing_hosts_admit_only_one_creation_for_an_agent_identity() {
    let fixture = Fixture::new();
    let first = fixture.host();
    let second = fixture.host();
    let one = Fixture::record("First", None);
    let mut two = one.clone();
    two.name = "Second".to_owned();
    let (left, right) = tokio::join!(first.ensure_agent(one), second.ensure_agent(two));
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let loser = left.err().or_else(|| right.err()).expect("one conflict");
    assert!(matches!(loser, LocalHostError::AgentConflict(_)));
    assert_eq!(first.list_agents().await.expect("one agent").len(), 1);
}

#[tokio::test]
async fn creation_rejects_missing_or_self_creators_and_unknown_profiles() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let parent = AgentId::new();
    assert!(
        matches!(host.ensure_agent(Fixture::record("News", Some(parent))).await, Err(LocalHostError::AgentNotFound(id)) if id == parent)
    );
    let mut record = Fixture::record("Self", None);
    record.created_by = Some(record.id);
    assert!(host.ensure_agent(record).await.is_err());
    let mut record = Fixture::record("Unknown", None);
    record.profile = AgentProfileId::new("unknown").expect("id");
    assert!(host.ensure_agent(record).await.is_err());
    assert!(
        host.ensure_agent(Fixture::record("  ", None))
            .await
            .is_err()
    );
    assert!(host.list_agents().await.expect("empty roster").is_empty());
}

#[tokio::test]
async fn one_agent_owns_multiple_isolated_sessions_across_restart_and_session_deletion() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let agent = host
        .ensure_agent(Fixture::record("Relay", None))
        .await
        .expect("agent");
    let first_id = Uuid::new_v4();
    let second_id = Uuid::new_v4();
    let first = host
        .ensure_agent_session(agent.id, &fixture.workspace, first_id)
        .await
        .expect("first session");
    let second = host
        .ensure_agent_session(agent.id, &fixture.workspace, second_id)
        .await
        .expect("second session");
    assert_eq!(first.agent_id(), agent.id);
    assert_eq!(second.agent_id(), agent.id);
    first
        .execute_turn(
            Uuid::new_v4(),
            vec![ContentBlock::text("First conversation")],
            Arc::new(NoopEvents),
        )
        .await
        .expect("real execution");
    assert!(second.history().expect("isolated history").is_empty());
    let expected = first.history().expect("first history");
    drop(first);
    drop(second);
    drop(host);
    let restarted = fixture.host();
    let restored = restarted
        .ensure_agent_session(agent.id, &fixture.workspace, first_id)
        .await
        .expect("restore exact identity");
    assert_eq!(restored.agent_id(), agent.id);
    assert_eq!(restored.history().expect("restored history"), expected);
    drop(restored);
    let other = restarted
        .ensure_agent(Fixture::record("Other", None))
        .await
        .expect("other agent");
    assert!(
        restarted
            .ensure_agent_session(other.id, &fixture.workspace, first_id)
            .await
            .is_err()
    );
    assert!(
        restarted
            .ensure_agent_session(AgentId::new(), &fixture.workspace, Uuid::new_v4())
            .await
            .is_err()
    );
    restarted
        .delete_session(first_id)
        .await
        .expect("delete one conversation");
    assert_eq!(
        restarted.agent(agent.id).await.expect("agent survives"),
        Some(agent)
    );
    let second = restarted
        .load_session(second_id, &fixture.workspace)
        .await
        .expect("other conversation survives");
    assert!(second.history().expect("still isolated").is_empty());
}

#[tokio::test]
async fn legacy_session_bindings_are_imported_without_opening_execution_or_replacing_names() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let profile = AgentProfileId::new(RELAY_PROFILE_ID).expect("profile");
    let session = host
        .create_session(&profile, &fixture.workspace)
        .await
        .expect("legacy creation API");
    let agent_id = session.agent_id();
    // Represent a published pre-catalog session (or a crash before catalog retention).
    let database = fixture.directory.path().join("data/host.sqlite3");
    Connection::open(database)
        .expect("catalog")
        .execute("DELETE FROM host_agents", [])
        .expect("remove catalog record");
    let conflicting = AgentRecord {
        id: agent_id,
        profile: AgentProfileId::new(renoa_local::ALPHA_PROFILE_ID).expect("other profile"),
        name: "Cannot take over existing identity".to_owned(),
        created_by: None,
    };
    assert!(
        matches!(host.ensure_agent(conflicting).await, Err(LocalHostError::AgentConflict(id)) if id == agent_id)
    );
    fs::remove_file(&fixture.bridge).expect("no model required");
    let roster = host
        .list_agents()
        .await
        .expect("import while kernel is owned");
    assert_eq!(roster.len(), 1);
    assert_eq!(roster[0].id, agent_id);
    assert_eq!(host.list_agents().await.expect("idempotent import"), roster);
    drop(session);
}

#[tokio::test]
async fn deleting_a_legacy_session_retains_its_agent_before_removing_the_manifest() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let profile = AgentProfileId::new(RELAY_PROFILE_ID).expect("profile");
    let session = host
        .create_session(&profile, &fixture.workspace)
        .await
        .expect("legacy session");
    let session_id = session.id();
    let agent_id = session.agent_id();
    drop(session);
    Connection::open(fixture.directory.path().join("data/host.sqlite3"))
        .expect("catalog")
        .execute("DELETE FROM host_agents", [])
        .expect("simulate pre-catalog publication");
    fs::remove_file(&fixture.bridge).expect("disable model execution");
    host.delete_session(session_id)
        .await
        .expect("delete before import");
    host.delete_session(session_id)
        .await
        .expect("retry deletion");
    assert!(
        !fixture
            .directory
            .path()
            .join("data/sessions")
            .join(session_id.to_string())
            .exists()
    );
    drop(host);
    let restarted = fixture.host();
    let retained = restarted
        .agent(agent_id)
        .await
        .expect("lookup")
        .expect("retained identity");
    assert_eq!(retained.profile, profile);
    assert_eq!(
        restarted.list_agents().await.expect("roster"),
        vec![retained]
    );
}

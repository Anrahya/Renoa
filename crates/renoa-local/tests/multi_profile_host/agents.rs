use super::*;
use renoa_local::derived_agent_id;

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
        )
    }
}

#[tokio::test]
async fn durable_roster_and_creator_relationship_survive_restart_without_model_dependencies() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let host_id = host.host_id().await.expect("Host identity");
    let parent = provision_specialist(&host, Uuid::new_v4(), "Operator", "Run the desk.").await;
    let child_creator = AgentCreator::Agent {
        agent_id: parent.id,
    };
    let child_operation = Uuid::new_v4();
    let child_request = AgentCreateRequest::new(
        child_operation,
        AgentPresetId::new(SPECIALIST_PRESET_ID).expect("preset id"),
        "News",
    )
    .with_instructions("Report the news.");
    let child = host
        .create_agent(
            child_creator.clone(),
            AgentCreationOrigin::AgentTool,
            child_request.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("child");
    assert_eq!(child.id, derived_agent_id(child_operation));
    assert_eq!(child.creator, child_creator);
    assert_eq!(
        host.create_agent(
            child_creator.clone(),
            AgentCreationOrigin::AgentTool,
            child_request.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("retry creation"),
        child
    );
    let changed = AgentCreateRequest::new(
        child_operation,
        AgentPresetId::new(SPECIALIST_PRESET_ID).expect("preset id"),
        "Other",
    )
    .with_instructions("Report the news.");
    assert!(
        matches!(host.create_agent(child_creator.clone(), AgentCreationOrigin::AgentTool, changed, CancellationToken::new()).await, Err(LocalHostError::AgentConflict(id)) if id == child.id)
    );
    drop(host);
    fs::remove_file(&fixture.bridge).expect("disable model dependency");
    let restarted = fixture.host();
    assert_eq!(restarted.host_id().await.expect("same host"), host_id);
    assert_eq!(
        restarted
            .agent_definition(child.id)
            .await
            .expect("inspect child"),
        Some(child.clone())
    );
    let agents = restarted.list_agents().await.expect("roster without model");
    assert_eq!(agents.len(), 2);
    assert!(agents.contains(&parent) && agents.contains(&child));
    assert_eq!(
        restarted
            .create_agent(
                child_creator,
                AgentCreationOrigin::AgentTool,
                child_request,
                CancellationToken::new(),
            )
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
    let operation = Uuid::new_v4();
    let creator = AgentCreator::System {
        component: "competing-hosts".to_owned(),
    };
    let one = AgentCreateRequest::new(
        operation,
        AgentPresetId::new(SPECIALIST_PRESET_ID).expect("preset id"),
        "First",
    )
    .with_instructions("Do the work.");
    let two = AgentCreateRequest::new(
        operation,
        AgentPresetId::new(SPECIALIST_PRESET_ID).expect("preset id"),
        "Second",
    )
    .with_instructions("Do the work.");
    let (left, right) = tokio::join!(
        first.create_agent(
            creator.clone(),
            AgentCreationOrigin::Provisioning,
            one,
            CancellationToken::new()
        ),
        second.create_agent(
            creator,
            AgentCreationOrigin::Provisioning,
            two,
            CancellationToken::new()
        )
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let loser = left.err().or_else(|| right.err()).expect("one conflict");
    assert!(matches!(loser, LocalHostError::AgentConflict(_)));
    assert_eq!(first.list_agents().await.expect("one agent").len(), 1);
}

#[tokio::test]
async fn creation_rejects_untrusted_pairs_and_unknown_presets() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let creator = AgentCreator::System {
        component: "validation".to_owned(),
    };
    assert!(matches!(
        host.create_agent(
            creator.clone(),
            AgentCreationOrigin::AgentTool,
            AgentCreateRequest::new(
                Uuid::new_v4(),
                AgentPresetId::new(SPECIALIST_PRESET_ID).expect("preset id"),
                "Untrusted"
            )
            .with_instructions("Do the work."),
            CancellationToken::new()
        )
        .await,
        Err(LocalHostError::InvalidRequest(_))
    ));
    assert!(matches!(
        host.create_agent(
            creator.clone(),
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::new(
                Uuid::nil(),
                AgentPresetId::new(SPECIALIST_PRESET_ID).expect("preset id"),
                "Nil"
            )
            .with_instructions("Do the work."),
            CancellationToken::new()
        )
        .await,
        Err(LocalHostError::InvalidRequest(_))
    ));
    assert!(matches!(
        host.create_agent(
            creator.clone(),
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::new(
                Uuid::new_v4(),
                AgentPresetId::new("renoa.unknown.v1").expect("preset id"),
                "Unknown"
            )
            .with_instructions("Do the work."),
            CancellationToken::new()
        )
        .await,
        Err(LocalHostError::Definition(_))
    ));
    assert!(matches!(
        host.create_agent(
            creator.clone(),
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::new(
                Uuid::new_v4(),
                AgentPresetId::new(SPECIALIST_PRESET_ID).expect("preset id"),
                "Without instructions"
            ),
            CancellationToken::new()
        )
        .await,
        Err(LocalHostError::Definition(_))
    ));
    assert!(matches!(
        host.create_agent(
            creator.clone(),
            AgentCreationOrigin::Provisioning,
            AgentCreateRequest::new(
                Uuid::new_v4(),
                AgentPresetId::new(SPECIALIST_PRESET_ID).expect("preset id"),
                "  "
            )
            .with_instructions("Do the work."),
            CancellationToken::new()
        )
        .await,
        Err(LocalHostError::Definition(_))
    ));
    assert!(host.list_agents().await.expect("empty roster").is_empty());
}

#[tokio::test]
async fn one_agent_owns_multiple_isolated_sessions_across_restart_and_session_deletion() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let agent = provision_specialist(&host, Uuid::new_v4(), "Relay", RELAY_PROMPT).await;
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
    let other =
        provision_specialist(&restarted, Uuid::new_v4(), "Other", "Do something else.").await;
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
        .delete_session(agent.id, first_id)
        .await
        .expect("delete one conversation");
    assert_eq!(
        restarted
            .agent_definition(agent.id)
            .await
            .expect("agent survives"),
        Some(agent.clone())
    );
    let second = restarted
        .load_session_for_agent(agent.id, second_id, &fixture.workspace)
        .await
        .expect("other conversation survives");
    assert!(second.history().expect("still isolated").is_empty());
}

#[tokio::test]
async fn session_bindings_carry_only_the_canonical_agent_identity() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let agent = provision_specialist(&host, Uuid::new_v4(), "Relay", RELAY_PROMPT).await;
    let session_id = Uuid::new_v4();
    let session = host
        .ensure_agent_session(agent.id, &fixture.workspace, session_id)
        .await
        .expect("session");
    assert_eq!(session.agent_id(), agent.id);
    drop(session);
    let manifest_path = fixture
        .directory
        .path()
        .join("data/sessions")
        .join(session_id.to_string())
        .join("session.json");
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest"))
            .expect("manifest JSON");
    assert_eq!(manifest["version"], 4);
    assert_eq!(manifest["agent_id"], agent.id.to_string());
    assert_eq!(manifest["session_id"], session_id.to_string());
    assert!(manifest.get("profile_id").is_none());
    fs::remove_file(&fixture.bridge).expect("no model required");
    let roster = host.list_agents().await.expect("roster without model");
    assert_eq!(roster, vec![agent]);
    assert_eq!(host.list_agents().await.expect("idempotent read"), roster);
}

#[tokio::test]
async fn loading_a_live_foreign_session_is_refused_before_its_kernel_opens() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let owner = provision_specialist(&host, Uuid::new_v4(), "Relay", RELAY_PROMPT).await;
    let intruder = provision_specialist(&host, Uuid::new_v4(), "Intruder", "Answer briefly.").await;
    let session_id = Uuid::new_v4();
    let live = host
        .ensure_agent_session(owner.id, &fixture.workspace, session_id)
        .await
        .expect("owner session");

    let load = host
        .load_session_for_agent(intruder.id, session_id, &fixture.workspace)
        .await
        .err()
        .expect("a foreign live session must be refused");
    assert!(
        matches!(&load, LocalHostError::InvalidRequest(message) if message == "session belongs to a different agent"),
        "unexpected load error: {load:?}"
    );

    let inspect = host
        .inspect_session(intruder.id, session_id, &fixture.workspace)
        .await
        .err()
        .expect("foreign history must be refused");
    assert!(
        matches!(&inspect, LocalHostError::InvalidRequest(message) if message == "session belongs to a different agent"),
        "unexpected inspect error: {inspect:?}"
    );
    drop(live);
}

#[tokio::test]
async fn deleting_a_foreign_agents_session_is_refused_and_retains_it() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let owner = provision_specialist(&host, Uuid::new_v4(), "Relay", RELAY_PROMPT).await;
    let intruder = provision_specialist(&host, Uuid::new_v4(), "Intruder", "Answer briefly.").await;
    let session_id = Uuid::new_v4();
    let session = host
        .ensure_agent_session(owner.id, &fixture.workspace, session_id)
        .await
        .expect("owner session");
    drop(session);

    let refused = host
        .delete_session(intruder.id, session_id)
        .await
        .expect_err("a foreign session must be refused");
    assert!(
        matches!(&refused, LocalHostError::InvalidRequest(message) if message == "session belongs to a different agent"),
        "unexpected delete error: {refused:?}"
    );
    assert!(
        fixture
            .directory
            .path()
            .join("data/sessions")
            .join(session_id.to_string())
            .is_dir(),
        "a refused delete removed the foreign session"
    );

    host.delete_session(intruder.id, Uuid::new_v4())
        .await
        .expect("an absent session stays idempotently deletable");
    host.delete_session(owner.id, session_id)
        .await
        .expect("the owner deletes the retained session");
    host.delete_session(owner.id, session_id)
        .await
        .expect("retried deletion stays idempotent");
}

#[tokio::test]
async fn deleting_a_session_retains_its_agent_before_removing_the_manifest() {
    let fixture = Fixture::new();
    let host = fixture.host();
    let agent = provision_specialist(&host, Uuid::new_v4(), "Relay", RELAY_PROMPT).await;
    let session_id = Uuid::new_v4();
    let session = host
        .ensure_agent_session(agent.id, &fixture.workspace, session_id)
        .await
        .expect("session");
    drop(session);
    fs::remove_file(&fixture.bridge).expect("disable model execution");
    host.delete_session(agent.id, session_id)
        .await
        .expect("delete session");
    host.delete_session(agent.id, session_id)
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
        .agent_definition(agent.id)
        .await
        .expect("lookup")
        .expect("retained identity");
    assert_eq!(retained, agent);
    assert_eq!(
        restarted.list_agents().await.expect("roster"),
        vec![retained]
    );
}

use std::{fs, path::Path};

use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    AgentCreateRequest, AgentRoutine, AgentToolsUpdate, MAX_AGENT_PAGE, RenameAgent,
    derived_agent_id,
};
use crate::{
    ARCEE_PROFILE_ID, AgentCreationOrigin, AgentCreator, AgentPresetId, AgentProfile, LocalHost,
    LocalHostError, ModelProvider,
    host::HostInitialization,
    host::routines::RoutineSchedule,
    presets::{ARCEE_PRESET_ID, SPECIALIST_PRESET_ID},
};

const VECTOR_OPERATION: Uuid = Uuid::from_u128(0x0001_0203_0405_0607_0809_0a0b_0c0d_0e0f);

fn host(root: &Path) -> LocalHost {
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
        profiles: vec![AgentProfile::new(ARCEE_PROFILE_ID, "Operator.").expect("profile")],
    })
    .expect("Host")
}

fn fixture() -> (tempfile::TempDir, LocalHost) {
    let directory = tempdir().expect("fixture");
    let root = directory.path();
    fs::create_dir(root.join("workspace")).expect("workspace");
    fs::write(root.join("model.mjs"), "// fixture\n").expect("model");
    fs::write(root.join("auth.sqlite"), "").expect("auth boundary");
    let host = host(root);
    (directory, host)
}

fn specialist(operation: Uuid, name: &str) -> AgentCreateRequest {
    AgentCreateRequest::new(
        operation,
        AgentPresetId::new(SPECIALIST_PRESET_ID).expect("preset id"),
        name,
    )
    .with_instructions("Do the assigned job.")
}

fn system(component: &str) -> (AgentCreator, AgentCreationOrigin) {
    (
        AgentCreator::System {
            component: component.to_owned(),
        },
        AgentCreationOrigin::Provisioning,
    )
}

#[tokio::test]
async fn creation_writes_one_canonical_definition_and_exact_selection() {
    let (_directory, host) = fixture();
    let (creator, origin) = system("test");
    let operation = Uuid::new_v4();
    let definition = host
        .create_agent(
            creator,
            origin,
            specialist(operation, "X Desk").with_tools(["read_file".to_owned()]),
            CancellationToken::new(),
        )
        .await
        .expect("create agent");

    assert_eq!(definition.id, derived_agent_id(operation));
    assert_eq!(definition.name, "X Desk");
    assert_eq!(definition.created_via, AgentCreationOrigin::Provisioning);
    assert_eq!(
        definition.preset_id,
        Some(AgentPresetId::new(SPECIALIST_PRESET_ID).expect("preset id"))
    );
    assert_eq!(definition.tool_selection.revision, 1);
    assert!(definition.tool_selection.tools.contains("read_file"));
    assert!(
        definition
            .tool_selection
            .tools
            .contains(crate::capabilities::ROUTINE_MANAGE),
        "the specialist baseline keeps routine management"
    );
    assert_eq!(definition.operational.instructions, "Do the assigned job.");
    assert_eq!(definition.operational.provider_restriction, None);
    assert!(definition.connections.is_empty());

    let stored = host
        .agent_definition(definition.id)
        .await
        .expect("read definition")
        .expect("definition exists");
    assert_eq!(stored, definition);
    assert_eq!(
        host.list_agent_definitions(None, MAX_AGENT_PAGE)
            .await
            .expect("list agents"),
        vec![definition.clone()]
    );
}

#[tokio::test]
async fn derived_identities_are_stable_across_releases() {
    assert_eq!(
        derived_agent_id(VECTOR_OPERATION).to_string(),
        "b752ff05-42c3-9fb4-ea04-f02fdbf33469"
    );

    let (_directory, host) = fixture();
    let (creator, origin) = system("test");
    let routine = AgentRoutine {
        name: "Morning".to_owned(),
        prompt: "Summarize.".to_owned(),
        schedule: RoutineSchedule::Interval { hours: 24 },
        enabled: true,
    };
    let definition = host
        .create_agent(
            creator,
            origin,
            specialist(VECTOR_OPERATION, "Scheduled").with_routine(routine),
            CancellationToken::new(),
        )
        .await
        .expect("create agent with a first routine");
    let routines = host
        .list_routines(definition.id, None)
        .await
        .expect("list routines");
    assert_eq!(routines.len(), 1);
    assert_eq!(
        routines[0].id.to_string(),
        "5795ef98-6b50-4219-6d80-57f795411276",
        "the routine id derives from its own domain string"
    );
}

#[tokio::test]
async fn creation_replays_exactly_and_conflicts_on_any_changed_field() {
    let (_directory, host) = fixture();
    let (creator, origin) = system("test");
    let operation = Uuid::new_v4();
    let request = specialist(operation, "Replay");
    let first = host
        .create_agent(
            creator.clone(),
            origin,
            request.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("create");

    let replay = host
        .create_agent(creator, origin, request, CancellationToken::new())
        .await
        .expect("identical replay");
    assert_eq!(replay, first, "a replay returns the stored result");

    // Same operation id, changed request.
    assert!(matches!(
        host.create_agent(
            AgentCreator::System {
                component: "test".to_owned()
            },
            origin,
            specialist(operation, "Renamed"),
            CancellationToken::new(),
        )
        .await,
        Err(LocalHostError::AgentConflict(_))
    ));

    // Same operation id, changed actor.
    assert!(matches!(
        host.create_agent(
            AgentCreator::System {
                component: "other".to_owned()
            },
            origin,
            specialist(operation, "Replay"),
            CancellationToken::new(),
        )
        .await,
        Err(LocalHostError::AgentConflict(_))
    ));

    assert_eq!(
        host.list_agent_definitions(None, MAX_AGENT_PAGE)
            .await
            .expect("list"),
        vec![first]
    );
}

#[tokio::test]
async fn validation_rejects_untrusted_pairs_unknown_names_and_preset_mismatches() {
    let (_directory, host) = fixture();
    let (creator, origin) = system("test");

    // An agent-tool origin cannot carry a provisioning actor.
    assert!(matches!(
        host.create_agent(
            creator.clone(),
            AgentCreationOrigin::AgentTool,
            specialist(Uuid::new_v4(), "Untrusted"),
            CancellationToken::new(),
        )
        .await,
        Err(LocalHostError::InvalidRequest(_))
    ));

    // A nil operation id is not a stable identity.
    assert!(matches!(
        host.create_agent(
            creator.clone(),
            origin,
            specialist(Uuid::nil(), "Nil"),
            CancellationToken::new(),
        )
        .await,
        Err(LocalHostError::InvalidRequest(_))
    ));

    // Unknown capability names are rejected, not dropped.
    assert!(matches!(
        host.create_agent(
            creator.clone(),
            origin,
            specialist(Uuid::new_v4(), "Unknown tool").with_tools(["all".to_owned()]),
            CancellationToken::new(),
        )
        .await,
        Err(LocalHostError::InvalidRequest(_))
    ));

    // An unregistered preset is rejected.
    let unknown = AgentCreateRequest::new(
        Uuid::new_v4(),
        AgentPresetId::new("renoa.unknown.v1").expect("preset id"),
        "Unknown preset",
    )
    .with_instructions("Do something.");
    assert!(matches!(
        host.create_agent(creator.clone(), origin, unknown, CancellationToken::new())
            .await,
        Err(LocalHostError::Definition(_))
    ));

    // A fixed-instruction preset rejects caller instructions.
    let fixed = AgentCreateRequest::new(
        Uuid::new_v4(),
        AgentPresetId::new(ARCEE_PRESET_ID).expect("preset id"),
        "Operator",
    )
    .with_instructions("Replace the curated prompt.");
    assert!(matches!(
        host.create_agent(creator.clone(), origin, fixed, CancellationToken::new())
            .await,
        Err(LocalHostError::Definition(_))
    ));

    // A caller-instruction preset requires them.
    let missing = AgentCreateRequest::new(
        Uuid::new_v4(),
        AgentPresetId::new(SPECIALIST_PRESET_ID).expect("preset id"),
        "No instructions",
    );
    assert!(matches!(
        host.create_agent(creator, origin, missing, CancellationToken::new())
            .await,
        Err(LocalHostError::Definition(_))
    ));

    assert!(
        host.list_agent_definitions(None, MAX_AGENT_PAGE)
            .await
            .expect("list")
            .is_empty(),
        "no rejected request may leave an agent"
    );
}

#[tokio::test]
async fn a_rejected_first_routine_leaves_no_agent_state() {
    let (_directory, host) = fixture();
    let (creator, origin) = system("test");
    let routine = AgentRoutine {
        name: String::new(),
        prompt: String::new(),
        schedule: RoutineSchedule::Interval { hours: 0 },
        enabled: true,
    };
    let result = host
        .create_agent(
            creator,
            origin,
            specialist(Uuid::new_v4(), "Broken routine").with_routine(routine),
            CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(result, Err(LocalHostError::Routine(_))),
        "an invalid routine must fail the creation: {result:?}"
    );
    assert!(
        host.list_agent_definitions(None, MAX_AGENT_PAGE)
            .await
            .expect("list")
            .is_empty(),
        "a failed creation must not leave an agent"
    );
}

#[tokio::test]
async fn concurrent_identical_creates_converge_on_one_agent() {
    let directory = tempdir().expect("fixture");
    let root = directory.path();
    fs::create_dir(root.join("workspace")).expect("workspace");
    fs::write(root.join("model.mjs"), "// fixture\n").expect("model");
    fs::write(root.join("auth.sqlite"), "").expect("auth boundary");
    let first_host = host(root);
    let second_host = host(root);
    let operation = Uuid::new_v4();
    let request = specialist(operation, "Concurrent");
    let (creator, origin) = system("test");

    let (first, second) = tokio::join!(
        first_host.create_agent(
            creator.clone(),
            origin,
            request.clone(),
            CancellationToken::new()
        ),
        second_host.create_agent(creator, origin, request, CancellationToken::new())
    );
    let first = first.expect("first create");
    let second = second.expect("second create");
    assert_eq!(first, second, "both callers observe one result");
    assert_eq!(
        second_host
            .list_agent_definitions(None, MAX_AGENT_PAGE)
            .await
            .expect("list"),
        vec![first]
    );
}

#[tokio::test]
async fn agents_from_one_preset_own_independent_selections() {
    let (_directory, host) = fixture();
    let (creator, origin) = system("test");
    let first = host
        .create_agent(
            creator.clone(),
            origin,
            specialist(Uuid::new_v4(), "First"),
            CancellationToken::new(),
        )
        .await
        .expect("first agent");
    let second = host
        .create_agent(
            creator,
            origin,
            specialist(Uuid::new_v4(), "Second"),
            CancellationToken::new(),
        )
        .await
        .expect("second agent");
    assert_ne!(first.id, second.id);

    let edited = host
        .set_agent_tools(AgentToolsUpdate {
            operation_id: Uuid::new_v4(),
            id: first.id,
            expected_revision: 1,
            tools: ["read_file".to_owned()].into_iter().collect(),
        })
        .await
        .expect("edit first selection");
    assert_eq!(edited.revision, 2);

    let untouched = host
        .agent_definition(second.id)
        .await
        .expect("read second")
        .expect("second exists");
    assert_eq!(untouched.tool_selection, second.tool_selection);
}

#[tokio::test]
async fn a_document_preset_publishes_files_and_records_provenance() {
    let (directory, host) = fixture();
    let (creator, origin) = system("test");
    let request = AgentCreateRequest::new(
        Uuid::new_v4(),
        AgentPresetId::new(ARCEE_PRESET_ID).expect("preset id"),
        "Operator",
    );
    let definition = host
        .create_agent(creator, origin, request, CancellationToken::new())
        .await
        .expect("create the operator agent");

    assert!(definition.operational.documents.is_some());
    assert_eq!(
        definition.operational.provider_restriction,
        Some(ModelProvider::OpenCodeGo)
    );
    let root = directory
        .path()
        .join("data")
        .join("agents")
        .join(definition.id.to_string());
    for file in ["SOUL.md", "USER.md"] {
        let metadata = fs::symlink_metadata(root.join(file)).expect("published document");
        assert!(metadata.file_type().is_file());
    }

    // Provenance is record data and never enters the instructions.
    assert!(
        !definition
            .operational
            .instructions
            .contains(&definition.id.to_string())
    );
    assert!(
        !definition
            .operational
            .instructions
            .contains(ARCEE_PRESET_ID),
        "the preset id is not prompt text"
    );
}

#[tokio::test]
async fn rename_requires_the_management_capability_and_replays() {
    let (_directory, host) = fixture();
    let (creator, origin) = system("test");
    let capable = host
        .create_agent(
            creator.clone(),
            origin,
            specialist(Uuid::new_v4(), "Capable")
                .with_tools([crate::capabilities::AGENT_MANAGE.to_owned()]),
            CancellationToken::new(),
        )
        .await
        .expect("capable agent");
    let plain = host
        .create_agent(
            creator.clone(),
            origin,
            specialist(Uuid::new_v4(), "Plain"),
            CancellationToken::new(),
        )
        .await
        .expect("plain agent");
    let target = host
        .create_agent(
            creator,
            origin,
            specialist(Uuid::new_v4(), "Target"),
            CancellationToken::new(),
        )
        .await
        .expect("target agent");

    let operation = Uuid::new_v4();
    let edit = RenameAgent {
        id: target.id,
        expected_name: "Target".to_owned(),
        name: "Renamed".to_owned(),
    };
    assert!(
        matches!(
            host.rename_agent(plain.id, operation, edit.clone(), CancellationToken::new())
                .await,
            Err(LocalHostError::InvalidRequest(_))
        ),
        "an agent without the management capability cannot rename another agent"
    );

    let renamed = host
        .rename_agent(
            capable.id,
            operation,
            edit.clone(),
            CancellationToken::new(),
        )
        .await
        .expect("capable rename");
    assert_eq!(renamed.name, "Renamed");
    assert_eq!(
        host.rename_agent(capable.id, operation, edit, CancellationToken::new())
            .await
            .expect("replay"),
        renamed
    );
    assert!(matches!(
        host.rename_agent(
            capable.id,
            Uuid::new_v4(),
            RenameAgent {
                id: target.id,
                expected_name: "Target".to_owned(),
                name: "Stale".to_owned(),
            },
            CancellationToken::new()
        )
        .await,
        Err(LocalHostError::AgentConflict(_))
    ));

    // An agent may always rename itself.
    let self_renamed = host
        .rename_agent(
            plain.id,
            Uuid::new_v4(),
            RenameAgent {
                id: plain.id,
                expected_name: "Plain".to_owned(),
                name: "Plain Renamed".to_owned(),
            },
            CancellationToken::new(),
        )
        .await
        .expect("self rename");
    assert_eq!(self_renamed.name, "Plain Renamed");
}

#[tokio::test]
async fn selection_edits_are_revision_checked_and_the_operational_document_stays_clean() {
    let (_directory, host) = fixture();
    let (creator, origin) = system("test");
    let definition = host
        .create_agent(
            creator,
            origin,
            specialist(Uuid::new_v4(), "Clean"),
            CancellationToken::new(),
        )
        .await
        .expect("create");

    let operation = Uuid::new_v4();
    let update = AgentToolsUpdate {
        operation_id: operation,
        id: definition.id,
        expected_revision: 1,
        tools: ["bash".to_owned()].into_iter().collect(),
    };
    let selection = host
        .set_agent_tools(update.clone())
        .await
        .expect("first edit");
    assert_eq!(selection.revision, 2);
    assert_eq!(
        host.set_agent_tools(update.clone()).await.expect("replay"),
        selection
    );
    // A fresh attempt with a stale revision conflicts.
    assert!(matches!(
        host.set_agent_tools(AgentToolsUpdate {
            operation_id: Uuid::new_v4(),
            expected_revision: 1,
            id: definition.id,
            tools: ["bash".to_owned()].into_iter().collect(),
        })
        .await,
        Err(LocalHostError::AgentConflict(_))
    ));

    // Selection state lives in its own tables, never inside the operational JSON.
    let stored = host
        .agent_definition(definition.id)
        .await
        .expect("read")
        .expect("exists");
    assert_eq!(stored.tool_selection, selection);
    let operational_json = serde_json::to_string(&stored.operational).expect("encode");
    assert!(!operational_json.contains("bash"));
    assert!(!operational_json.contains("read_file"));
    assert!(!operational_json.contains("SOUL.md"));
}

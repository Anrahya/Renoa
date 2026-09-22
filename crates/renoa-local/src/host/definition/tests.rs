use std::{collections::BTreeSet, fs, path::Path};

use tempfile::tempdir;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    AgentCreateRequest, AgentRoutine, AgentToolsUpdate, MAX_AGENT_PAGE, RenameAgent,
    derived_agent_id,
};
use crate::{
    AgentCreationOrigin, AgentCreator, AgentPresetId, LocalHost, LocalHostError, ModelProvider,
    host::HostInitialization,
    host::routines::RoutineSchedule,
    presets::{ALPHA_PRESET_ID, ARCEE_PRESET_ID, SPECIALIST_PRESET_ID},
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

struct PresetExpectation {
    preset_id: &'static str,
    agent_name: &'static str,
    instructions: Option<&'static str>,
    capabilities: &'static [&'static str],
}

#[tokio::test]
async fn preset_capability_baselines_keep_the_existing_exact_runtime_selections() {
    let (_directory, host) = fixture();
    let (creator, origin) = system("test");
    let cases = [
        PresetExpectation {
            preset_id: ALPHA_PRESET_ID,
            agent_name: "Alpha",
            instructions: None,
            capabilities: &[
                "bash",
                "edit_file",
                "extension_manage",
                "find",
                "git_changes",
                "git_diff",
                "git_show",
                "grep",
                "read_file",
                "skill_load",
                "skill_search",
                "tool_execute",
                "tool_load",
                "tool_search",
                "write_file",
            ],
        },
        PresetExpectation {
            preset_id: ARCEE_PRESET_ID,
            agent_name: "Arcee",
            instructions: None,
            capabilities: &[
                "agent_documents",
                "agent_manage",
                "bash",
                "edit_file",
                "extension_manage",
                "find",
                "git_changes",
                "git_diff",
                "git_show",
                "grep",
                "read_file",
                "routine_manage",
                "routine_results",
                "skill_load",
                "skill_search",
                "tool_execute",
                "tool_load",
                "tool_search",
                "write_file",
            ],
        },
        PresetExpectation {
            preset_id: SPECIALIST_PRESET_ID,
            agent_name: "Specialist",
            instructions: Some("Do the assigned job."),
            capabilities: &[
                "routine_manage",
                "routine_results",
                "skill_load",
                "skill_search",
                "tool_execute",
                "tool_load",
                "tool_search",
            ],
        },
    ];
    for case in cases {
        let mut request = AgentCreateRequest::new(
            Uuid::new_v4(),
            AgentPresetId::new(case.preset_id).expect("preset id"),
            case.agent_name,
        );
        if let Some(instructions) = case.instructions {
            request = request.with_instructions(instructions);
        }
        let definition = host
            .create_agent(creator.clone(), origin, request, CancellationToken::new())
            .await
            .expect("create from capability-backed preset");
        let expected: BTreeSet<String> = case
            .capabilities
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        assert_eq!(
            definition.tool_selection.tools, expected,
            "preset `{}`",
            case.preset_id
        );
    }
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
    let page = host
        .list_agent_definitions(None, MAX_AGENT_PAGE)
        .await
        .expect("list agents");
    assert_eq!(page.agents, vec![definition.clone()]);
    assert_eq!(page.next_cursor, None);
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
        .list_routines(definition.id, definition.id, None)
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
        .create_agent(
            creator.clone(),
            origin,
            request.clone(),
            CancellationToken::new(),
        )
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

    let page = host
        .list_agent_definitions(None, MAX_AGENT_PAGE)
        .await
        .expect("list");
    assert_eq!(page.agents, vec![first.clone()]);
    assert_eq!(page.next_cursor, None);

    // A later rename changes the live agent and leaves the stored creation
    // result alone, so the same operation still replays the created definition.
    host.rename_agent(
        first.id,
        Uuid::new_v4(),
        RenameAgent {
            id: first.id,
            expected_name: first.name.clone(),
            name: "Renamed".to_owned(),
        },
        CancellationToken::new(),
    )
    .await
    .expect("rename the created agent");
    let after_rename = host
        .create_agent(creator, origin, request, CancellationToken::new())
        .await
        .expect("replay after a rename");
    assert_eq!(
        after_rename, first,
        "a replay returns the stored creation result, not the current definition"
    );
    assert_eq!(
        host.agent_definition(first.id)
            .await
            .expect("read the renamed agent")
            .expect("the agent exists")
            .name,
        "Renamed"
    );
}

/// A selection cannot name a capability the definition cannot exercise: the
/// runtime would drop the binding while the stored selection still named it.
#[tokio::test]
async fn a_selection_cannot_name_a_capability_the_definition_cannot_consume() {
    let (_directory, host) = fixture();
    let (creator, origin) = system("test");
    let rejection = "keeps documents";
    let rejected = host
        .create_agent(
            creator.clone(),
            origin,
            specialist(Uuid::new_v4(), "Without documents")
                .with_tools(["agent_documents".to_owned()]),
            CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(&rejected, Err(LocalHostError::InvalidRequest(message)) if message.contains(rejection)),
        "a definition without documents cannot keep the document capability: {rejected:?}"
    );

    let definition = host
        .create_agent(
            creator.clone(),
            origin,
            specialist(Uuid::new_v4(), "Without tools"),
            CancellationToken::new(),
        )
        .await
        .expect("create");
    let edited = host
        .set_agent_tools(AgentToolsUpdate {
            operation_id: Uuid::new_v4(),
            id: definition.id,
            expected_revision: 1,
            tools: ["agent_documents".to_owned(), "read_file".to_owned()]
                .into_iter()
                .collect(),
        })
        .await;
    assert!(
        matches!(&edited, Err(LocalHostError::InvalidRequest(message)) if message.contains(rejection)),
        "a later edit cannot add an unconsumable capability: {edited:?}"
    );

    let enabled = host
        .create_agent(
            creator,
            origin,
            AgentCreateRequest::new(
                Uuid::new_v4(),
                AgentPresetId::new(ARCEE_PRESET_ID).expect("preset id"),
                "With documents",
            )
            .with_tools(["agent_documents".to_owned()]),
            CancellationToken::new(),
        )
        .await
        .expect("a document-enabled agent keeps the document capability");
    assert!(
        enabled.tool_selection.tools.contains("agent_documents"),
        "the document capability belongs to the stored selection"
    );
}
/// A stored selection naming a capability the definition cannot use must fail
/// closed at the read boundary instead of being dropped at runtime.
#[tokio::test]
async fn a_stored_selection_the_definition_cannot_use_is_refused() {
    let (directory, host) = fixture();
    let (creator, origin) = system("test");
    let definition = host
        .create_agent(
            creator,
            origin,
            specialist(Uuid::new_v4(), "Tampered selection"),
            CancellationToken::new(),
        )
        .await
        .expect("create");
    let database = crate::host::catalog::open_verified(&directory.path().join("data/host.sqlite3"))
        .expect("open the Host catalog");
    for tools in [r#"["agent_documents"]"#, r#"["renoa.bot.manage"]"#] {
        database
            .execute(
                "UPDATE host_agent_tool_selections SET tools_json = ?2 WHERE agent_id = ?1",
                [definition.id.to_string(), tools.to_owned()],
            )
            .expect("tamper with the stored selection");
        let error = host
            .agent_definition(definition.id)
            .await
            .expect_err("an unusable stored capability must fail closed");
        assert!(
            error.to_string().contains("unusable capability"),
            "unexpected error for {tools}: {error}"
        );
    }
}

/// A creation receipt that describes a different agent must fail closed rather
/// than replay a definition this operation never created.
#[tokio::test]
async fn a_creation_receipt_that_describes_another_agent_is_refused() {
    let (directory, host) = fixture();
    let (creator, origin) = system("test");
    let operation = Uuid::new_v4();
    let request = specialist(operation, "Receipt");
    host.create_agent(
        creator.clone(),
        origin,
        request.clone(),
        CancellationToken::new(),
    )
    .await
    .expect("create");
    {
        let database =
            crate::host::catalog::open_verified(&directory.path().join("data/host.sqlite3"))
                .expect("open the Host catalog");
        database
            .execute(
                "UPDATE host_agent_creations SET result_json = json_set(result_json, '$.id', ?1)
                 WHERE operation_id = ?2",
                [Uuid::new_v4().to_string(), operation.to_string()],
            )
            .expect("tamper with the stored creation result");
    }
    let error = host
        .create_agent(creator, origin, request, CancellationToken::new())
        .await
        .expect_err("a receipt describing another agent must fail closed");
    assert!(
        error.to_string().contains("describes agent"),
        "unexpected error: {error}"
    );
}

/// A stored definition that fails validation must be refused at the read
/// boundary instead of reaching the runtime.
#[tokio::test]
async fn a_stored_definition_that_fails_validation_is_refused() {
    let (directory, host) = fixture();
    let (creator, origin) = system("test");
    let definition = host
        .create_agent(
            creator,
            origin,
            specialist(Uuid::new_v4(), "Corrupted"),
            CancellationToken::new(),
        )
        .await
        .expect("create");
    {
        let database =
            crate::host::catalog::open_verified(&directory.path().join("data/host.sqlite3"))
                .expect("open the Host catalog");
        database
            .execute(
                "UPDATE host_agents SET name = ' padded ' WHERE agent_id = ?1",
                [definition.id.to_string()],
            )
            .expect("corrupt the stored name");
    }
    let error = host
        .agent_definition(definition.id)
        .await
        .expect_err("a semantically invalid stored definition must fail closed");
    assert!(
        error.to_string().contains("is invalid"),
        "unexpected error: {error}"
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
            .agents
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
            .agents
            .is_empty(),
        "a failed creation must not leave an agent"
    );
}

/// A creation rejected for an unusable connection must not publish the preset's
/// documents either: publication is the last step before the commit.
#[tokio::test]
async fn a_creation_that_fails_validation_publishes_no_documents() {
    let (directory, host) = fixture();
    let (creator, origin) = system("test");
    let operation = Uuid::new_v4();
    let request = AgentCreateRequest::new(
        operation,
        AgentPresetId::new(ARCEE_PRESET_ID).expect("preset id"),
        "Unusable connection",
    )
    .with_connections(vec!["unknown.integration".to_owned()]);
    let result = host
        .create_agent(creator, origin, request, CancellationToken::new())
        .await;
    assert!(
        matches!(result, Err(LocalHostError::Mcp(_))),
        "a creation naming an unusable connection must fail: {result:?}"
    );
    let documents = directory
        .path()
        .join("data/agents")
        .join(derived_agent_id(operation).to_string());
    assert!(
        !documents.exists(),
        "a rejected creation must not publish documents: {documents:?}"
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
    let page = second_host
        .list_agent_definitions(None, MAX_AGENT_PAGE)
        .await
        .expect("list");
    assert_eq!(page.agents, vec![first]);
    assert_eq!(page.next_cursor, None);
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

mod management;
mod resolution;

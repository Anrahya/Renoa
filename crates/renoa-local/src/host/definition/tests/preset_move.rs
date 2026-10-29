use super::*;
use renoa_kernel::AgentId;

use crate::{AgentDefinition, AgentDefinitionError, WorkspaceInstructions, presets};

fn soul_path(directory: &tempfile::TempDir, agent: &AgentDefinition) -> std::path::PathBuf {
    directory
        .path()
        .join("data/agents")
        .join(agent.id.to_string())
        .join("SOUL.md")
}

fn app() -> AgentPresetId {
    AgentPresetId::new(APP_PRESET_ID).expect("preset id")
}

/// A general agent the way the app made it before `renoa.app.v1`.
async fn general_agent(host: &LocalHost, operation: Uuid) -> AgentDefinition {
    let (creator, origin) = system("test");
    let request = AgentCreateRequest {
        purpose: Some("Count my calories.".to_owned()),
        ..specialist(operation, "Renoa").with_tools(["read_file".to_owned()])
    };
    host.create_agent(creator, origin, request, CancellationToken::new())
        .await
        .expect("create a general agent")
}

#[tokio::test]
async fn a_moved_agent_takes_the_preset_seed_and_keeps_everything_it_owns() {
    let (directory, host) = fixture();
    let operation = Uuid::new_v4();
    let before = general_agent(&host, operation).await;

    let moved = host
        .move_agent_to_preset(before.id, app())
        .await
        .expect("move");
    assert!(moved.changed);
    let after = moved.definition;
    let seed = presets::preset(&app()).expect("preset").seed();
    assert_eq!(after.preset_id, Some(app()));
    assert_eq!(after.operational.instructions, seed.instructions);
    assert_eq!(after.operational.documents, seed.documents);
    assert_eq!(
        after.operational.behavior.workspace_instructions,
        WorkspaceInstructions::Off
    );
    let kept = AgentDefinition {
        preset_id: before.preset_id.clone(),
        operational: crate::AgentOperationalDefinition {
            instructions: before.operational.instructions.clone(),
            behavior: before.operational.behavior,
            documents: before.operational.documents,
            ..after.operational.clone()
        },
        ..after.clone()
    };
    assert_eq!(kept, before, "only the preset seed changed");
    assert_eq!(
        host.agent_definition(before.id).await.expect("read"),
        Some(after.clone())
    );
    assert_eq!(
        fs::read_to_string(soul_path(&directory, &after)).expect("published SOUL.md"),
        seed.soul.expect("default soul")
    );
    let prompt = host
        .resolve_definition(after.id)
        .await
        .expect("resolve")
        .system_prompt(&directory.path().join("workspace"))
        .expect("prompt");
    assert!(prompt.contains("<soul source=\"SOUL.md\""), "{prompt}");

    // The creation record stays: replaying the creation returns what it made.
    let (creator, origin) = system("test");
    let request = AgentCreateRequest {
        purpose: Some("Count my calories.".to_owned()),
        ..specialist(operation, "Renoa").with_tools(["read_file".to_owned()])
    };
    let replayed = host
        .create_agent(creator, origin, request, CancellationToken::new())
        .await
        .expect("replay the creation");
    assert_eq!(replayed, before);
}

#[tokio::test]
async fn an_agent_already_on_the_preset_is_left_untouched() {
    let (directory, host) = fixture();
    let agent = general_agent(&host, Uuid::new_v4()).await;
    let first = host
        .move_agent_to_preset(agent.id, app())
        .await
        .expect("move");
    let soul = soul_path(&directory, &agent);
    fs::write(&soul, "The owner's own words.\n").expect("owner edit");

    let again = host
        .move_agent_to_preset(agent.id, app())
        .await
        .expect("move again");
    assert!(!again.changed);
    assert_eq!(again.definition, first.definition);
    assert_eq!(
        fs::read_to_string(soul).expect("soul"),
        "The owner's own words.\n"
    );
}

#[tokio::test]
async fn an_unknown_agent_or_preset_changes_nothing() {
    let (directory, host) = fixture();
    let missing = AgentId::new();
    let refused = host.move_agent_to_preset(missing, app()).await;
    assert!(
        matches!(refused, Err(LocalHostError::AgentNotFound(id)) if id == missing),
        "{refused:?}"
    );
    assert!(
        !directory
            .path()
            .join("data/agents")
            .join(missing.to_string())
            .exists()
    );

    let agent = general_agent(&host, Uuid::new_v4()).await;
    let unknown = AgentPresetId::new("renoa.unknown.v1").expect("portable id");
    let refused = host.move_agent_to_preset(agent.id, unknown).await;
    assert!(
        matches!(
            refused,
            Err(LocalHostError::Definition(
                AgentDefinitionError::UnknownPreset { .. }
            ))
        ),
        "{refused:?}"
    );
    assert_eq!(
        host.agent_definition(agent.id).await.expect("read"),
        Some(agent.clone())
    );
    assert!(!soul_path(&directory, &agent).exists());
}

#[tokio::test]
async fn an_existing_soul_is_kept() {
    let (directory, host) = fixture();
    let (creator, origin) = system("test");
    let operator = host
        .create_agent(
            creator,
            origin,
            AgentCreateRequest::from_preset(
                Uuid::new_v4(),
                AgentPresetId::new(ARCEE_PRESET_ID).expect("preset id"),
                "Operator",
            ),
            CancellationToken::new(),
        )
        .await
        .expect("create an agent with a soul");
    let soul = soul_path(&directory, &operator);
    let written = fs::read_to_string(&soul).expect("arcee soul");

    let moved = host
        .move_agent_to_preset(operator.id, app())
        .await
        .expect("move");
    assert!(moved.changed);
    assert_eq!(fs::read_to_string(&soul).expect("soul"), written);
    assert_ne!(
        Some(written.as_str()),
        presets::preset(&app()).expect("preset").seed().soul
    );
}

#[tokio::test]
async fn a_failed_commit_leaves_no_residue_and_the_retry_converges() {
    let (directory, host) = fixture();
    let agent = general_agent(&host, Uuid::new_v4()).await;
    let root = directory
        .path()
        .join("data/agents")
        .join(agent.id.to_string());
    let root_existed = root.exists();

    super::super::commit_fault::arm(agent.id);
    let failed = host.move_agent_to_preset(agent.id, app()).await;
    assert!(
        matches!(failed, Err(LocalHostError::HostCatalog(_))),
        "{failed:?}"
    );
    assert_eq!(
        host.agent_definition(agent.id).await.expect("read"),
        Some(agent.clone()),
        "the row is unchanged"
    );
    assert!(
        !root.join("SOUL.md").exists(),
        "the published soul is withdrawn"
    );
    assert_eq!(root.exists(), root_existed);

    let moved = host
        .move_agent_to_preset(agent.id, app())
        .await
        .expect("the retry converges");
    assert!(moved.changed);
    assert!(root.join("SOUL.md").is_file());
}

/// An agent already on the preset whose SOUL.md went missing, for example
/// after an earlier run as another user was cleaned up, gets it back.
#[tokio::test]
async fn a_repeated_move_restores_a_missing_soul() {
    let (directory, host) = fixture();
    let agent = general_agent(&host, Uuid::new_v4()).await;
    let first = host
        .move_agent_to_preset(agent.id, app())
        .await
        .expect("move");
    let soul = soul_path(&directory, &agent);
    fs::remove_file(&soul).expect("lose SOUL.md");

    let repaired = host
        .move_agent_to_preset(agent.id, app())
        .await
        .expect("repair");
    assert!(repaired.changed, "the restored SOUL.md is a change");
    assert_eq!(repaired.definition, first.definition);
    assert_eq!(
        fs::read_to_string(&soul).expect("restored SOUL.md"),
        presets::preset(&app())
            .expect("preset")
            .seed()
            .soul
            .expect("default soul")
    );
    let again = host
        .move_agent_to_preset(agent.id, app())
        .await
        .expect("no-op");
    assert!(!again.changed);
}

#[tokio::test]
async fn a_caller_that_does_not_own_the_data_directory_is_refused_first() {
    let (directory, host) = fixture();
    let agent = general_agent(&host, Uuid::new_v4()).await;
    let effective = nix::unistd::geteuid().as_raw();
    super::super::preset_move::owner_fault::pretend(
        &directory.path().join("data"),
        effective.wrapping_add(1),
    );

    let refused = host.move_agent_to_preset(agent.id, app()).await;
    assert!(
        matches!(&refused, Err(LocalHostError::InvalidRequest(message))
            if message.contains("owner of the Host data directory")),
        "{refused:?}"
    );
    assert_eq!(
        host.agent_definition(agent.id).await.expect("read"),
        Some(agent.clone())
    );
    assert!(!soul_path(&directory, &agent).exists());
}

/// An existing SOUL.md the Host cannot read fails the move closed: neither the
/// row nor the file changes.
#[tokio::test]
async fn an_unreadable_soul_fails_closed() {
    let (directory, host) = fixture();
    let agent = general_agent(&host, Uuid::new_v4()).await;
    let soul = soul_path(&directory, &agent);
    fs::create_dir_all(soul.parent().expect("agent directory")).expect("agent directory");
    fs::write(&soul, [0xff, 0xfe, b'\n']).expect("not UTF-8");

    let refused = host.move_agent_to_preset(agent.id, app()).await;
    assert!(
        matches!(refused, Err(LocalHostError::Definition(_))),
        "{refused:?}"
    );
    assert_eq!(
        host.agent_definition(agent.id).await.expect("read"),
        Some(agent.clone())
    );
    assert_eq!(fs::read(&soul).expect("SOUL.md"), [0xff, 0xfe, b'\n']);
}

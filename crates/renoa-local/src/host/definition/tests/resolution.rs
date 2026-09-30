use super::*;
use crate::host::definition::resolve_definition;

#[tokio::test]
async fn a_document_preset_publishes_files_and_records_provenance() {
    let (directory, host) = fixture();
    let (creator, origin) = system("test");
    let request = AgentCreateRequest::from_preset(
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
    let metadata = fs::symlink_metadata(root.join("SOUL.md")).expect("published soul");
    assert!(metadata.file_type().is_file());
    assert!(
        !root.join("USER.md").exists(),
        "USER.md belongs to a person, never to an agent"
    );

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
async fn resolution_composes_the_stored_definition_with_workspace_rules() {
    let (directory, host) = fixture();
    let (creator, origin) = system("test");
    let operation = Uuid::new_v4();
    let specialist = host
        .create_agent(
            creator.clone(),
            origin,
            specialist(operation, "Resolved").with_tools(["read_file".to_owned()]),
            CancellationToken::new(),
        )
        .await
        .expect("create a caller-instruction agent");

    let workspace = directory.path().join("workspace");
    let resolved = resolve_definition(&host.config, specialist.id)
        .await
        .expect("resolve the stored definition");
    assert_eq!(resolved.agent_id(), specialist.id);
    assert_eq!(resolved.provider_restriction(), None);
    assert!(resolved.document_binding().is_none());
    assert_eq!(
        resolved.system_prompt(&workspace).expect("compose prompt"),
        "Do the assigned job.",
        "resolution uses the stored instructions and never a preset"
    );

    // A document-backed preset composes documents and workspace rules.
    let operator = host
        .create_agent(
            system("test").0,
            system("test").1,
            AgentCreateRequest::from_preset(
                Uuid::new_v4(),
                AgentPresetId::new(ARCEE_PRESET_ID).expect("preset id"),
                "Operator",
            ),
            CancellationToken::new(),
        )
        .await
        .expect("create the operator agent");
    fs::write(workspace.join("AGENTS.md"), "Keep the public API small.\n")
        .expect("write project instructions");
    let resolved = resolve_definition(&host.config, operator.id)
        .await
        .expect("resolve the operator definition");
    assert_eq!(
        resolved.provider_restriction(),
        Some(ModelProvider::OpenCodeGo)
    );
    assert!(resolved.automatic_compaction().is_some());
    assert!(resolved.document_binding().is_some());
    let prompt = resolved.system_prompt(&workspace).expect("compose prompt");
    assert!(prompt.contains("source=\"SOUL.md\""));
    assert!(
        !prompt.contains("source=\"USER.md\""),
        "without a person the prompt has no USER.md"
    );
    assert!(prompt.contains("<project_instructions source=\"AGENTS.md\">"));
    assert!(prompt.contains("Keep the public API small."));
    let prompt = resolved
        .with_principal(Some(Uuid::new_v4()))
        .system_prompt(&workspace)
        .expect("compose prompt for a person");
    assert!(prompt.contains("source=\"USER.md\""));
}

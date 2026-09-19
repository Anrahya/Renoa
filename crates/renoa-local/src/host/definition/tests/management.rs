use super::*;
use crate::RenameAgent;

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

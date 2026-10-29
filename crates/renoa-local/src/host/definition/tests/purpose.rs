use super::*;
use crate::host::definition::resolve_definition;

fn with_purpose(request: AgentCreateRequest, purpose: &str) -> AgentCreateRequest {
    AgentCreateRequest {
        purpose: Some(purpose.to_owned()),
        ..request
    }
}

#[tokio::test]
async fn a_purpose_is_kept_in_the_definition_and_read_by_the_agent() {
    let (directory, host) = fixture();
    let (creator, origin) = system("test");
    let definition = host
        .create_agent(
            creator,
            origin,
            with_purpose(
                specialist(Uuid::new_v4(), "Kcal"),
                "Count my calories from photos.\nNudge me after lunch.\n",
            ),
            CancellationToken::new(),
        )
        .await
        .expect("create");
    assert_eq!(
        definition.operational.purpose.as_deref(),
        Some("Count my calories from photos.\nNudge me after lunch.\n"),
        "kept as the owner wrote it"
    );
    let stored = host
        .agent_definition(definition.id)
        .await
        .expect("read")
        .expect("exists");
    assert_eq!(stored, definition);

    let resolved = resolve_definition(&host.config, definition.id)
        .await
        .expect("resolve");
    assert_eq!(
        resolved
            .system_prompt(&directory.path().join("workspace"))
            .expect("prompt"),
        "Do the assigned job.\n\n<identity>\nYour name is Kcal.\n</identity>\n\n<purpose>\nWhat you were made for:\nCount my calories from photos.\nNudge me after lunch.\n</purpose>"
    );
}

#[tokio::test]
async fn a_purpose_that_is_blank_too_long_or_not_plain_text_makes_no_agent() {
    let (_directory, host) = fixture();
    let refused = [
        String::new(),
        "  \n ".to_owned(),
        "a".repeat(1025),
        "tab\there".to_owned(),
        "nul\u{0}here".to_owned(),
        "next line\u{85}here".to_owned(),
    ];
    for purpose in refused {
        let operation = Uuid::new_v4();
        let (creator, origin) = system("test");
        let outcome = host
            .create_agent(
                creator,
                origin,
                with_purpose(specialist(operation, "Refused"), &purpose),
                CancellationToken::new(),
            )
            .await;
        let Err(LocalHostError::Definition(error)) = outcome else {
            panic!("{purpose:?} was not refused: {outcome:?}");
        };
        assert!(
            error
                .to_string()
                .contains(&format!("received {} bytes", purpose.len())),
            "the refusal says what it received: {error}"
        );
        assert_eq!(
            host.agent_definition(derived_agent_id(operation))
                .await
                .expect("read"),
            None,
            "a refused purpose leaves no agent"
        );
    }

    let (creator, origin) = system("test");
    let longest = "a".repeat(1024);
    let kept = host
        .create_agent(
            creator,
            origin,
            with_purpose(specialist(Uuid::new_v4(), "Longest"), &longest),
            CancellationToken::new(),
        )
        .await
        .expect("1,024 bytes is a purpose");
    assert_eq!(kept.operational.purpose, Some(longest));
}

#[tokio::test]
async fn a_request_without_a_purpose_is_stored_as_before_and_one_with_it_replays() {
    let request = specialist(Uuid::new_v4(), "Before");
    let json = serde_json::to_value(&request).expect("request JSON");
    assert!(
        json.get("purpose").is_none(),
        "a request without a purpose serializes as before, so it replays byte for byte"
    );
    let definition = serde_json::to_value(crate::AgentOperationalDefinition {
        instructions: "Old.".to_owned(),
        purpose: None,
        behavior: crate::AgentBehavior {
            workspace_instructions: crate::WorkspaceInstructions::Off,
            automatic_compaction: None,
        },
        documents: None,
        provider_restriction: None,
        model: None,
    })
    .expect("definition JSON");
    assert!(definition.get("purpose").is_none());

    let (_directory, host) = fixture();
    let (creator, origin) = system("test");
    let operation = Uuid::new_v4();
    let request = with_purpose(specialist(operation, "Replay"), "Read the news.");
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
        .create_agent(creator.clone(), origin, request, CancellationToken::new())
        .await
        .expect("replay");
    assert_eq!(replay, first);
    assert!(
        matches!(
            host.create_agent(
                creator,
                origin,
                with_purpose(specialist(operation, "Replay"), "Read the weather."),
                CancellationToken::new(),
            )
            .await,
            Err(LocalHostError::AgentConflict(_))
        ),
        "the same operation with another purpose is a conflict"
    );
}

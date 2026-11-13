use super::*;
use crate::{host::definition::resolve_definition, model_context::estimate_input_tokens, presets};

#[tokio::test]
async fn a_job_is_kept_after_purpose_and_leaves_the_preset_base_first() {
    let (directory, host) = fixture();
    let (creator, origin) = system("test");
    let request = AgentCreateRequest {
        purpose: Some("Track my daily reading.".to_owned()),
        ..AgentCreateRequest::from_preset(
            Uuid::new_v4(),
            AgentPresetId::new(APP_PRESET_ID).expect("preset id"),
            "Reader",
        )
        .with_job("Summarize saved links every evening.\nKeep quotes exact.\n")
    };
    let definition = host
        .create_agent(creator, origin, request, CancellationToken::new())
        .await
        .expect("create");
    let app_seed = presets::preset(&AgentPresetId::new(APP_PRESET_ID).expect("preset"))
        .expect("preset")
        .seed();
    assert_eq!(definition.operational.instructions, app_seed.instructions);
    assert_eq!(
        definition.operational.job.as_deref(),
        Some("Summarize saved links every evening.\nKeep quotes exact.\n")
    );
    let stored = host
        .agent_definition(definition.id)
        .await
        .expect("read")
        .expect("exists");
    assert_eq!(stored, definition);

    let prompt = resolve_definition(&host.config, definition.id)
        .await
        .expect("resolve")
        .system_prompt(&directory.path().join("workspace"))
        .expect("prompt");
    assert!(
        prompt.starts_with(app_seed.instructions.trim_end()),
        "{prompt}"
    );
    let identity_pos = prompt
        .find("<identity>\nYour name is Reader.\n</identity>")
        .expect("identity");
    let purpose_pos = prompt
        .find("<purpose>\nWhat you were made for:\nTrack my daily reading.\n</purpose>")
        .expect("purpose");
    let job_pos = prompt
        .find("<job>\nSummarize saved links every evening.\nKeep quotes exact.\n</job>")
        .expect("job");
    let soul_pos = prompt.find("<soul source=\"SOUL.md\"").expect("soul");
    assert!(
        identity_pos < purpose_pos && purpose_pos < job_pos && job_pos < soul_pos,
        "{prompt}"
    );
}

#[tokio::test]
async fn a_job_that_is_blank_too_long_or_not_plain_text_makes_no_agent() {
    let (directory, host) = fixture();
    let max = crate::agent_definition::MAX_JOB_BYTES;
    let refused = [
        String::new(),
        "  \n ".to_owned(),
        "a".repeat(max + 1),
        "tab\there".to_owned(),
        "nul\u{0}here".to_owned(),
        "next line\u{85}here".to_owned(),
    ];
    for job in refused {
        let operation = Uuid::new_v4();
        let (creator, origin) = system("test");
        let outcome = host
            .create_agent(
                creator,
                origin,
                AgentCreateRequest::from_preset(
                    operation,
                    AgentPresetId::new(APP_PRESET_ID).expect("preset id"),
                    "Refused",
                )
                .with_job(&job),
                CancellationToken::new(),
            )
            .await;
        let Err(LocalHostError::Definition(error)) = outcome else {
            panic!("{job:?} was not refused: {outcome:?}");
        };
        assert!(
            error
                .to_string()
                .contains(&format!("received {} bytes", job.len())),
            "the refusal says what it received: {error}"
        );
        assert_eq!(
            host.agent_definition(derived_agent_id(operation))
                .await
                .expect("read"),
            None
        );
        assert!(
            !directory
                .path()
                .join("data/agents")
                .join(derived_agent_id(operation).to_string())
                .exists(),
            "a refused job publishes no documents"
        );
    }

    let (creator, origin) = system("test");
    let longest = "a".repeat(max);
    let kept = host
        .create_agent(
            creator,
            origin,
            AgentCreateRequest::from_preset(
                Uuid::new_v4(),
                AgentPresetId::new(APP_PRESET_ID).expect("preset id"),
                "Longest",
            )
            .with_job(&longest),
            CancellationToken::new(),
        )
        .await
        .expect("max job bytes accepted");
    assert_eq!(kept.operational.job, Some(longest));
}

#[tokio::test]
async fn a_legacy_row_without_job_loads_unchanged_and_omits_job_block() {
    let (directory, host) = fixture();
    let (creator, origin) = system("test");
    let request = specialist(Uuid::new_v4(), "Legacy");
    let request_json = serde_json::to_value(&request).expect("request JSON");
    assert!(request_json.get("job").is_none());

    let definition = host
        .create_agent(creator, origin, request, CancellationToken::new())
        .await
        .expect("create");
    let encoded = serde_json::to_value(&definition.operational).expect("definition JSON");
    assert!(encoded.get("job").is_none());

    // Simulate a pre-job stored row missing the "job" key in SQLite.
    rusqlite::Connection::open(&host.config.database)
        .expect("catalog")
        .execute(
            "UPDATE host_agents SET operational_json = json_remove(operational_json, '$.job') WHERE agent_id = ?1",
            [definition.id.to_string()],
        )
        .expect("strip job");
    let reloaded = host
        .agent_definition(definition.id)
        .await
        .expect("read")
        .expect("exists");
    assert_eq!(reloaded, definition);
    let prompt = resolve_definition(&host.config, definition.id)
        .await
        .expect("resolve")
        .system_prompt(&directory.path().join("workspace"))
        .expect("prompt");
    assert!(!prompt.contains("<job>"), "{prompt}");
}

#[test]
fn app_agent_with_max_bounded_prompt_and_job_fits_compaction_tail_budget() {
    let seed = presets::preset(&AgentPresetId::new(APP_PRESET_ID).expect("preset"))
        .expect("preset")
        .seed();
    let compaction = seed
        .behavior
        .automatic_compaction
        .expect("app preset compaction");
    let max_summary =
        crate::runtime::MAX_CHECKPOINT_TOKENS.min(compaction.target_input_tokens.get() / 4);
    let tail_budget = compaction.target_input_tokens.get() - max_summary;

    let max_prompt = format!(
        "{}\n\n<identity>\nYour name is {}.\n</identity>\n\n<purpose>\nWhat you were made for:\n{}\n</purpose>\n\n<job>\n{}\n</job>\n\n<soul source=\"SOUL.md\" revision=\"{}\">\n{}\n</soul>\n\n<session_context>\n<context source=\"plugin:renoa.memory\" revision=\"{}\">\n{}\n</context>\n</session_context>",
        seed.instructions.trim_end(),
        "n".repeat(crate::agent_definition::MAX_NAME_BYTES),
        "p".repeat(crate::agent_definition::MAX_PURPOSE_BYTES),
        "j".repeat(crate::agent_definition::MAX_JOB_BYTES),
        "0".repeat(64),
        seed.soul.unwrap_or_default(),
        "0".repeat(64),
        "m".repeat(crate::plugins::host::memory::SESSION_BLOCK_MAX),
    );
    let request = renoa_agent::ModelRequest {
        system_prompt: max_prompt,
        messages: Vec::new(),
        tools: (0..19)
            .map(|i| renoa_agent::ToolSpec {
                name: format!("arcee_tool_{i}"),
                description: "d".repeat(1_228),
                input_schema: serde_json::json!({ "type": "object" }),
            })
            .collect(),
    };
    let fixed_tokens = estimate_input_tokens(&request);
    assert!(
        fixed_tokens < tail_budget,
        "fixed overhead ({fixed_tokens} tok) must leave room inside tail_budget ({tail_budget} tok)"
    );
}

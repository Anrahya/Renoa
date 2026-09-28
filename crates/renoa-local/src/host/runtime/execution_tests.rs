use std::{fs, path::PathBuf, sync::Arc};

use renoa_agent::{AgentEvent, AgentEventSink, BoxFuture, ContentBlock, ToolCall, ToolOutput};
use renoa_agent_loop::AgentToolBinding;
use renoa_kernel::SessionId;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::protocol_bindings;
use crate::{
    AgentCreateRequest, AgentCreationOrigin, AgentCreator, AgentDocuments, AgentPresetId,
    LocalHost, LocalHostAdapters, LocalModelConfiguration, LocalTurnOutcome, ModelProvider,
    ReasoningLevel, TurnObservation,
};

struct Quiet;
impl AgentEventSink for Quiet {
    fn emit(&self, _: AgentEvent) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}
fn host(root: &std::path::Path, worker: Option<&std::path::Path>) -> LocalHost {
    fs::write(root.join("model.mjs"), include_str!("model.mjs")).expect("model");
    fs::write(root.join("auth.sqlite"), "").expect("credential boundary");
    LocalHost::new(
        root.join("home"),
        LocalModelConfiguration::new(
            root.join("model.mjs"),
            vec![ModelProvider::Xai],
            ModelProvider::Xai,
            "fallback",
            root.join("auth.sqlite"),
        ),
        LocalHostAdapters::new(None).with_code_mode_worker(worker),
    )
    .expect("Host")
}
async fn create(host: &LocalHost, request: AgentCreateRequest) -> crate::AgentDefinition {
    host.create_agent(
        AgentCreator::System {
            component: "protocol-fixture".to_owned(),
        },
        AgentCreationOrigin::Provisioning,
        request,
        CancellationToken::new(),
    )
    .await
    .expect("create")
}
async fn invoke(
    bindings: &[AgentToolBinding],
    name: &str,
    id: &str,
    arguments: Value,
) -> Result<ToolOutput, renoa_agent::ToolError> {
    let tool = bindings
        .iter()
        .find(|binding| binding.tool_name() == name)
        .expect("binding")
        .tool();
    let result = renoa_agent::invoke_tool(
        Some(tool.as_ref()),
        ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments,
            namespace: None,
            thought_signature: None,
        },
        CancellationToken::new(),
        None,
    )
    .await
    .expect("known outcome");
    if result.is_error {
        return Err(renoa_agent::ToolError::invalid_input(
            "plugin rejected invocation",
        ));
    }
    Ok(ToolOutput {
        content: result.content,
        details: result.details,
        is_error: false,
    })
}
fn value(output: &ToolOutput) -> Value {
    let [ContentBlock::Text { text }] = output.content.as_slice() else {
        panic!("JSON result")
    };
    serde_json::from_str(text).expect("JSON")
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one real protocol scenario follows discovery, creation receipt replay, grant isolation, and live disabling"
)]
async fn discovered_host_plugins_create_children_without_changing_the_callers_machine_grants() {
    let directory = tempfile::tempdir().expect("fixture");
    let host = host(directory.path(), None);
    let parent = create(
        &host,
        AgentCreateRequest::new(Uuid::new_v4(), "Parent", "Manage agents."),
    )
    .await;
    let definition = host.resolve_definition(parent.id).await.expect("resolve");
    let workspace = host.agent_workspace(parent.id).await.expect("workspace");
    let bindings = protocol_bindings(
        &host.config,
        &definition,
        &workspace,
        SessionId::from_uuid(Uuid::new_v4()),
        None,
        Vec::new(),
    )
    .expect("protocol");
    assert_eq!(
        bindings
            .iter()
            .map(AgentToolBinding::tool_name)
            .collect::<Vec<_>>(),
        ["plugin_search", "plugin_manage", "tool_execute"]
    );
    let browse = value(
        &invoke(&bindings, "plugin_search", "browse", json!({"query":"*"}))
            .await
            .expect("browse"),
    );
    assert_eq!(browse["total"], 5);
    assert!(
        browse["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|card| card.get("input_schema").is_none())
    );
    let nested = value(
        &invoke(
            &bindings,
            "plugin_search",
            "inspect",
            json!({"plugin":"renoa.agents"}),
        )
        .await
        .expect("inspect"),
    );
    let reference = nested["items"][0]["reference"].as_str().expect("reference");
    assert!(nested["items"][0].get("input_schema").is_none());
    let exact = value(
        &invoke(
            &bindings,
            "plugin_search",
            "schema",
            json!({"reference":reference}),
        )
        .await
        .expect("schema"),
    );
    assert_eq!(
        exact["input_schema"]["properties"]["tools"]["items"]["enum"],
        json!([
            "read_file",
            "edit_file",
            "write_file",
            "bash",
            "grep",
            "find"
        ])
    );
    let args = json!({"reference":reference,"arguments":{"action":"create","name":"Child","instructions":"Review the repository.","tools":["read_file"]}});
    let child = invoke(&bindings, "tool_execute", "create-once", args.clone())
        .await
        .expect("create via plugin");
    assert_eq!(value(&child)["tools"], json!(["read_file"]));
    assert_eq!(
        value(
            &invoke(&bindings, "tool_execute", "create-once", args)
                .await
                .expect("replay")
        ),
        value(&child)
    );
    assert_eq!(host.list_agents().await.expect("agents").len(), 2);
    assert!(
        host.agent_definition(parent.id)
            .await
            .unwrap()
            .unwrap()
            .tool_selection
            .tools
            .is_empty()
    );
    let child_id = renoa_kernel::AgentId::from_uuid(
        Uuid::parse_str(value(&child)["id"].as_str().unwrap()).unwrap(),
    );
    let child_definition = host.resolve_definition(child_id).await.unwrap();
    assert_eq!(
        protocol_bindings(
            &host.config,
            &child_definition,
            &workspace,
            SessionId::from_uuid(Uuid::new_v4()),
            None,
            Vec::new()
        )
        .unwrap()
        .len(),
        3
    );
    assert!(
        invoke(
            &bindings,
            "tool_execute",
            "self-grant",
            json!({"reference":reference,"arguments":{"action":"set_tools","tools":["bash"]}})
        )
        .await
        .is_err()
    );
    invoke(
        &bindings,
        "plugin_manage",
        "disable",
        json!({"action":"deactivate","plugin_id":"renoa.agents"}),
    )
    .await
    .expect("disable");
    assert!(
        invoke(
            &bindings,
            "tool_execute",
            "blocked",
            json!({"reference":reference,"arguments":{"action":"list"}})
        )
        .await
        .is_err()
    );
    invoke(
        &bindings,
        "plugin_manage",
        "enable",
        json!({"action":"enable_plugin","plugin_id":"renoa.agents"}),
    )
    .await
    .expect("enable");
    assert!(
        invoke(
            &bindings,
            "tool_execute",
            "enabled",
            json!({"reference":reference,"arguments":{"action":"list"}})
        )
        .await
        .is_ok()
    );
    let stale = reference.replacen(reference.split(':').nth(2).unwrap(), &"0".repeat(64), 1);
    assert!(
        invoke(
            &bindings,
            "tool_execute",
            "stale",
            json!({"reference":stale,"arguments":{"action":"list"}})
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn explicit_creation_overrides_template_tools_and_consumes_its_model_default() {
    let directory = tempfile::tempdir().expect("fixture");
    let host = host(directory.path(), None);
    let request = AgentCreateRequest::from_preset(
        Uuid::new_v4(),
        AgentPresetId::new(crate::presets::ALPHA_PRESET_ID).unwrap(),
        "Custom",
    )
    .with_instructions("Use these caller instructions.")
    .with_tools(Vec::new())
    .with_model(crate::AgentModelSelection {
        provider: ModelProvider::Xai,
        model: "selected".to_owned(),
        reasoning: Some(ReasoningLevel::High),
    });
    let agent = create(&host, request.clone()).await;
    assert_eq!(
        agent.operational.instructions,
        "Use these caller instructions."
    );
    assert!(agent.tool_selection.tools.is_empty());
    let workspace = host.agent_workspace(agent.id).await.unwrap();
    let session = host
        .ensure_agent_session(agent.id, &workspace, Uuid::new_v4())
        .await
        .expect("model session");
    let config = session.configuration().unwrap();
    assert_eq!(config.model, "xai/selected");
    assert_eq!(config.reasoning, ReasoningLevel::High);
    fs::remove_file(directory.path().join("model.mjs")).unwrap();
    assert_eq!(
        create(&host, request).await,
        agent,
        "receipt replay must not require a new model catalog"
    );
}

async fn run_creation_through_model(worker: Option<&std::path::Path>) {
    let directory = tempfile::tempdir().expect("fixture");
    let host = host(directory.path(), worker);
    let parent = create(
        &host,
        AgentCreateRequest::new(Uuid::new_v4(), "Parent", "Create an agent using plugins."),
    )
    .await;
    let workspace = host.agent_workspace(parent.id).await.unwrap();
    let session = host
        .ensure_agent_session(parent.id, &workspace, Uuid::new_v4())
        .await
        .unwrap();
    let request = Uuid::new_v4();
    let prompt = vec![ContentBlock::text("Create a child without machine tools.")];
    let result = session
        .execute_turn(request, prompt.clone(), Arc::new(Quiet))
        .await
        .expect("execute");
    assert!(
        matches!(&result,LocalTurnOutcome::Completed {output,..} if output=="Child created without machine access.")
    );
    assert_eq!(host.list_agents().await.unwrap().len(), 2);
    assert_eq!(
        session
            .execute_turn(request, prompt, Arc::new(Quiet))
            .await
            .unwrap(),
        result
    );
    assert_eq!(host.list_agents().await.unwrap().len(), 2);
}
#[tokio::test]
async fn direct_plugin_protocol_completes_an_agent_creation_turn() {
    run_creation_through_model(None).await;
}
#[tokio::test]
async fn monty_invokes_host_plugins_without_an_mcp_adapter() {
    let Some(worker) = std::env::var_os("RENOA_TEST_MONTY_WORKER").map(PathBuf::from) else {
        return;
    };
    run_creation_through_model(Some(&worker)).await;
}

#[tokio::test]
async fn monty_skill_activation_reattaches_one_body_without_expanding_the_next_turn() {
    let Some(worker) = std::env::var_os("RENOA_TEST_MONTY_WORKER").map(PathBuf::from) else {
        return;
    };
    let directory = tempfile::tempdir().expect("fixture");
    let host = host(directory.path(), Some(&worker));
    let parent = create(
        &host,
        AgentCreateRequest::new(Uuid::new_v4(), "Parent", "Use discovered skills."),
    )
    .await;
    let workspace = host.agent_workspace(parent.id).await.unwrap();
    let skill = workspace.join(".agents/skills/review");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: review\ndescription: Review guidance\n---\nPINNED_REVIEW_INSTRUCTION\n",
    )
    .unwrap();
    let session = host
        .ensure_agent_session(parent.id, &workspace, Uuid::new_v4())
        .await
        .unwrap();
    for (prompt, expected) in [
        ("Activate a review skill.", "Review skill activated."),
        (
            "Check current review guidance.",
            "Review guidance appears once.",
        ),
    ] {
        let outcome = session
            .execute_turn(
                Uuid::new_v4(),
                vec![ContentBlock::text(prompt)],
                Arc::new(Quiet),
            )
            .await
            .expect("skill turn");
        assert!(
            matches!(&outcome, LocalTurnOutcome::Completed {output,..} if output==expected),
            "{prompt}: {outcome:?}; history={:?}",
            session.history()
        );
    }
    assert!(session.history().unwrap().iter().any(|entry| matches!(&entry.message,renoa_agent::Message::Tool {result} if result.name=="code_mode" && result.content.iter().any(|content|matches!(content,ContentBlock::Text {text} if text.contains("PINNED_REVIEW_INSTRUCTION"))))),"projection must preserve the full durable result");
}

#[tokio::test]
async fn a_turn_reads_the_profile_of_the_person_it_comes_from() {
    let directory = tempfile::tempdir().expect("fixture");
    let host = host(directory.path(), None);
    let mut request = AgentCreateRequest::new(Uuid::new_v4(), "Profiled", "Answer the person.");
    request.documents = Some(AgentDocuments {
        soul: true,
        user: true,
    });
    let agent = create(&host, request).await;
    let owner = Uuid::new_v4();
    let profile = host.home().path().join("users").join(owner.to_string());
    fs::create_dir_all(&profile).expect("profile directory");
    fs::write(profile.join("USER.md"), "PROFILE_OWNER\n").expect("profile");
    let workspace = host.agent_workspace(agent.id).await.unwrap();
    for (principal, seen) in [
        (Some(owner), "PROFILE_OWNER"),
        (Some(Uuid::new_v4()), "none"),
        (None, "none"),
    ] {
        // One session per person, as RCP gives each task a single principal.
        let session = host
            .ensure_agent_session(agent.id, &workspace, Uuid::new_v4())
            .await
            .unwrap();
        let outcome = session
            .execute_turn_observed_with_cancellation(
                Uuid::new_v4(),
                vec![ContentBlock::text("Which profile do you see?")],
                TurnObservation::now().expect("time"),
                Arc::new(Quiet),
                CancellationToken::new(),
                principal,
            )
            .await
            .expect("execute");
        assert!(
            matches!(&outcome, LocalTurnOutcome::Completed { output, .. } if output == seen),
            "{principal:?} saw {outcome:?}"
        );
    }
}
